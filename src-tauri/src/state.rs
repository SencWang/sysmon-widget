//! 共享状态：最近快照、历史队列、远程主机运行态与配置。
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::config::AppConfig;
use crate::model::{HistoryPayload, HostRuntime, Snapshot};

/// 单台远程主机的运行态 + 历史曲线。
pub struct RemoteEntry {
    pub runtime: HostRuntime,
    pub history: VecDeque<Snapshot>,
}

pub struct Inner {
    pub snapshot: Option<Snapshot>,
    pub history: VecDeque<Snapshot>,
    pub config: AppConfig,
    /// key = host.id（`ssh:<alias>`）
    pub remotes: BTreeMap<String, RemoteEntry>,
    /// `refresh_remote` 手动触发的待采集主机
    pub pending_refresh: HashSet<String>,
}

pub struct AppState {
    pub inner: Mutex<Inner>,
    pub config_path: PathBuf,
}

impl AppState {
    pub fn new(config: AppConfig, config_path: PathBuf) -> Self {
        Self {
            inner: Mutex::new(Inner {
                snapshot: None,
                history: VecDeque::new(),
                config,
                remotes: BTreeMap::new(),
                pending_refresh: HashSet::new(),
            }),
            config_path,
        }
    }

    /// 后端窗口尺寸/视图切换依赖的配置视图名。
    pub fn config(&self) -> AppConfig {
        match self.inner.lock() {
            Ok(inner) => inner.config.clone(),
            Err(_) => AppConfig::default(),
        }
    }

    /// 按配置顺序返回启用的远程主机运行态（未采集过的补一条离线占位）。
    pub fn remotes(&self) -> Vec<HostRuntime> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        inner
            .config
            .servers
            .iter()
            .filter(|server| server.enabled)
            .map(|server| match inner.remotes.get(&server.id) {
                Some(entry) => entry.runtime.clone(),
                None => HostRuntime {
                    host: crate::model::HostInfo {
                        id: server.id.clone(),
                        label: label_of(server),
                        kind: crate::model::HOST_KIND_SSH.to_string(),
                    },
                    online: false,
                    latency_ms: None,
                    last_ok_ts: None,
                    snapshot: None,
                    error: Some("等待首次采集…".to_string()),
                },
            })
            .collect()
    }

    /// 写入一次成功采样：更新运行态与历史，并按 historyPoints 截断。
    pub fn apply_remote_success(&self, id: &str, snapshot: Snapshot, latency_ms: u64, cap: usize) {
        if let Ok(mut inner) = self.inner.lock() {
            let entry = inner.remotes.entry(id.to_string()).or_insert_with(|| RemoteEntry {
                runtime: HostRuntime {
                    host: snapshot.host.clone(),
                    online: true,
                    latency_ms: None,
                    last_ok_ts: None,
                    snapshot: None,
                    error: None,
                },
                history: VecDeque::new(),
            });
            entry.runtime.host = snapshot.host.clone();
            entry.runtime.online = true;
            entry.runtime.latency_ms = Some(latency_ms);
            entry.runtime.last_ok_ts = Some(snapshot.ts);
            entry.runtime.snapshot = Some(snapshot.clone());
            entry.runtime.error = None;
            entry.history.push_back(snapshot);
            while entry.history.len() > cap.max(1) {
                entry.history.pop_front();
            }
        }
    }

    /// 写入一次失败：保留最近成功快照，仅更新在线状态与错误信息。
    pub fn apply_remote_error(&self, id: &str, host: crate::model::HostInfo, error: String) {
        if let Ok(mut inner) = self.inner.lock() {
            let entry = inner.remotes.entry(id.to_string()).or_insert_with(|| RemoteEntry {
                runtime: HostRuntime {
                    host: host.clone(),
                    online: false,
                    latency_ms: None,
                    last_ok_ts: None,
                    snapshot: None,
                    error: None,
                },
                history: VecDeque::new(),
            });
            entry.runtime.host = host;
            entry.runtime.online = false;
            entry.runtime.error = Some(error);
        }
    }

    pub fn remote_history(&self, id: &str) -> HistoryPayload {
        match self.inner.lock() {
            Ok(inner) => match inner.remotes.get(id) {
                Some(entry) => crate::collector::build_history(&entry.history),
                None => HistoryPayload::default(),
            },
            Err(_) => HistoryPayload::default(),
        }
    }

    /// 预置远程运行态（线程启动时先显示“连接中”）；已存在时同步最新显示名。
    pub fn ensure_remote(&self, host: crate::model::HostInfo) {
        if let Ok(mut inner) = self.inner.lock() {
            let entry = inner
                .remotes
                .entry(host.id.clone())
                .or_insert_with(|| RemoteEntry {
                    runtime: HostRuntime {
                        host: host.clone(),
                        online: false,
                        latency_ms: None,
                        last_ok_ts: None,
                        snapshot: None,
                        error: Some("正在连接…".to_string()),
                    },
                    history: VecDeque::new(),
                });
            entry.runtime.host = host;
        }
    }

    /// 移除已删除主机的运行态与历史，避免内存泄漏。
    pub fn prune_remotes(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            let keep: HashSet<String> = inner
                .config
                .servers
                .iter()
                .map(|server| server.id.clone())
                .collect();
            inner.remotes.retain(|id, _| keep.contains(id));
        }
    }

    pub fn mark_refresh(&self, id: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.pending_refresh.insert(id.to_string());
        }
    }

    pub fn take_refresh(&self, id: &str) -> bool {
        match self.inner.lock() {
            Ok(mut inner) => inner.pending_refresh.remove(id),
            Err(_) => false,
        }
    }

    /// 托盘 tooltip 摘要：本机 CPU / 内存 + 在线主机数（m = 本机 + 已启用远程）。
    /// 托盘线程每 5 秒取一次，设置面板也展示同一文本便于核对。
    pub fn tray_text(&self) -> String {
        let snapshot = self
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.snapshot.clone());
        let remotes = self.remotes();
        let cfg = self.config();
        let cpu = snapshot.as_ref().map(|s| s.cpu.usage).unwrap_or(0.0);
        let mem = snapshot
            .as_ref()
            .map(|s| crate::model::percent(s.mem.used, s.mem.total))
            .unwrap_or(0.0);
        let total = 1 + cfg.servers.iter().filter(|server| server.enabled).count();
        let online =
            remotes.iter().filter(|entry| entry.online).count() + usize::from(snapshot.is_some());
        crate::model::tray_summary(cpu, mem, online, total)
    }
}

/// 显示名缺省用 alias。
pub fn label_of(server: &crate::config::ServerConfig) -> String {
    if server.label.trim().is_empty() {
        server.alias.clone()
    } else {
        server.label.clone()
    }
}

/// 窗口尺寸（逻辑像素）：tile 420x280 / detail 900x620 / grid 780x520。
pub fn view_size(view: &str) -> (f64, f64) {
    match view {
        "detail" => (900.0, 620.0),
        "grid" => (780.0, 520.0),
        _ => (420.0, 280.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CpuInfo, HostInfo, MemInfo, Snapshot};

    fn snap(host: HostInfo, cpu: f64, mem_used: u64, mem_total: u64) -> Snapshot {
        Snapshot {
            host,
            ts: 1,
            cpu: CpuInfo {
                usage: cpu,
                per_core: Vec::new(),
                cores: 8,
            },
            mem: MemInfo {
                used: mem_used,
                total: mem_total,
            },
            gpus: Vec::new(),
            processes: Vec::new(),
            error: None,
            gpu_proc_supported: false,
        }
    }

    #[test]
    fn view_sizes_are_fixed() {
        // 0.2.1：tile 放大字号后加宽到 420x280，仍为固定尺寸
        assert_eq!(view_size("tile"), (420.0, 280.0));
        assert_eq!(view_size("detail"), (900.0, 620.0));
        assert_eq!(view_size("grid"), (780.0, 520.0));
        // 未知视图回退到 tile
        assert_eq!(view_size("nope"), (420.0, 280.0));
    }

    #[test]
    fn readability_tokens_and_tile_size_are_enforced() {
        // 0.2.1 可读性心约：字号阶梯 / 文字颜色 / tile 尺寸不再散落魔法数字
        let css = include_str!("../../src/styles.css");
        assert!(css.contains("--fs-xs: 11.5px"));
        assert!(css.contains("--fs-sm: 13px"));
        assert!(css.contains("--fs-md: 14px"));
        assert!(css.contains("--fs-2xl: 26px"));
        assert!(css.contains("--fg: #f0f6fc"));
        assert!(css.contains("--fg-muted: #c8d3e0"));
        assert!(css.contains("--fg-dim: #aeb9c6"));
        // 主机名 16px/600、环形数字用 26px 变量、进程表 13px
        assert!(css.contains("font-size: 16px"));
        assert!(css.contains(".ring-value"));
        assert!(css.contains(".proc-table"));
        assert!(css.contains("font-size: var(--fs-md)"));
        // 放大字号后 tile 扩到 420x280，其余视图保持不变
        assert_eq!(view_size("tile"), (420.0, 280.0));
        assert_eq!(view_size("detail"), (900.0, 620.0));
        assert_eq!(view_size("grid"), (780.0, 520.0));
    }

    #[test]
    fn tray_text_counts_local_plus_enabled_remotes() {
        let mut cfg = AppConfig::default();
        cfg.servers = vec![
            crate::config::ServerConfig {
                id: "ssh:a".into(),
                label: "a".into(),
                alias: "a".into(),
                enabled: true,
            },
            crate::config::ServerConfig {
                id: "ssh:b".into(),
                label: "b".into(),
                alias: "b".into(),
                enabled: true,
            },
            crate::config::ServerConfig {
                id: "ssh:c".into(),
                label: "c".into(),
                alias: "c".into(),
                enabled: false,
            },
        ];
        let state = AppState::new(cfg, PathBuf::from("config.json"));
        // 还没有任何采样：0 在线，m = 本机 + 2 台启用远程
        assert_eq!(state.tray_text(), "本机 CPU 0% · 内存 0% · 在线 0/3");

        {
            let mut inner = state.inner.lock().unwrap();
            inner.snapshot = Some(snap(HostInfo::local("本机"), 12.4, 74, 100));
        }
        state.apply_remote_success(
            "ssh:a",
            snap(
                HostInfo {
                    id: "ssh:a".into(),
                    label: "a".into(),
                    kind: crate::model::HOST_KIND_SSH.into(),
                },
                5.0,
                1,
                2,
            ),
            120,
            180,
        );
        let text = state.tray_text();
        assert_eq!(text, "本机 CPU 12% · 内存 74% · 在线 2/3");
        assert!(text.chars().count() <= 128);
    }
}
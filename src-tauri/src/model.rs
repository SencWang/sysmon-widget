//! 数据模型：Rust 与前端共享的 Snapshot 结构（serde camelCase）。
use serde::{Deserialize, Serialize};

/// 主机类型：本期仅本机，下一阶段增加 SSH 远端。
pub const HOST_KIND_LOCAL: &str = "local";
pub const HOST_KIND_SSH: &str = "ssh";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub id: String,
    pub label: String,
    pub kind: String,
}

impl HostInfo {
    pub fn local(label: &str) -> Self {
        Self {
            id: "local".to_string(),
            label: label.to_string(),
            kind: HOST_KIND_LOCAL.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CpuInfo {
    /// 全局使用率 0-100
    pub usage: f64,
    /// 每核使用率 0-100
    pub per_core: Vec<f64>,
    pub cores: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemInfo {
    /// 字节
    pub used: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    /// 0-100
    pub util: f64,
    /// 字节
    pub mem_used: u64,
    pub mem_total: u64,
    pub temp: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    /// 可能超过 100（多核并行）
    pub cpu: f64,
    /// 字节
    pub mem: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub host: HostInfo,
    /// epoch ms
    pub ts: i64,
    pub cpu: CpuInfo,
    pub mem: MemInfo,
    pub gpus: Vec<GpuInfo>,
    pub processes: Vec<ProcessInfo>,
    pub error: Option<String>,
}

/// 主机运行态：本机与远程统一结构，前端 grid/detail 共用。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostRuntime {
    pub host: HostInfo,
    pub online: bool,
    pub latency_ms: Option<u64>,
    pub last_ok_ts: Option<i64>,
    /// 最近一次成功采集（离线时保留，便于界面展示旧数据）
    pub snapshot: Option<Snapshot>,
    pub error: Option<String>,
}

/// 历史曲线数据（由后端聚合，前端只负责绘制）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPayload {
    pub ts: Vec<i64>,
    pub cpu: Vec<f64>,
    pub mem: Vec<f64>,
    pub gpu: Vec<f64>,
    pub vram: Vec<f64>,
}

/// 阈值等级：<60 正常，60-85 警告，>85 危险。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Level {
    Normal,
    Warn,
    Danger,
}

/// 百分比计算，total 为 0 时返回 0，结果夹在 0-100。
pub fn percent(used: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    (used as f64 * 100.0 / total as f64).clamp(0.0, 100.0)
}

/// 使用率 -> 等级阈值（前端也有等价实现，这里保留给后端复用与测试）。
#[allow(dead_code)]
pub fn level_for(usage: f64) -> Level {
    if usage > 85.0 {
        Level::Danger
    } else if usage >= 60.0 {
        Level::Warn
    } else {
        Level::Normal
    }
}

/// 当前时间（epoch ms）。
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(_) => 0,
    }
}

/// 按字符（不是字节）截断，避免中文被切坏。
pub fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// 托盘 tooltip 摘要：`本机 CPU 12% · 内存 74% · 在线 2/3`，不超过 128 字符。
pub fn tray_summary(local_cpu: f64, mem_percent: f64, online: usize, total: usize) -> String {
    let cpu = if local_cpu.is_finite() {
        local_cpu.clamp(0.0, 100.0)
    } else {
        0.0
    };
    let mem = if mem_percent.is_finite() {
        mem_percent.clamp(0.0, 100.0)
    } else {
        0.0
    };
    truncate_chars(
        &format!(
            "本机 CPU {cpu:.0}% · 内存 {mem:.0}% · 在线 {online}/{}",
            total.max(1)
        ),
        128,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_and_threshold_calculation() {
        assert_eq!(percent(0, 0), 0.0);
        assert_eq!(percent(1, 0), 0.0);
        assert!((percent(512, 1024) - 50.0).abs() < f64::EPSILON);
        assert_eq!(percent(4096, 1024), 100.0);

        assert_eq!(level_for(0.0), Level::Normal);
        assert_eq!(level_for(59.9), Level::Normal);
        assert_eq!(level_for(60.0), Level::Warn);
        assert_eq!(level_for(85.0), Level::Warn);
        assert_eq!(level_for(85.1), Level::Danger);
        assert_eq!(level_for(100.0), Level::Danger);
    }

    #[test]
    fn snapshot_serializes_camel_case() {
        let snap = Snapshot {
            host: HostInfo::local("本机"),
            ts: 1,
            cpu: CpuInfo {
                usage: 12.5,
                per_core: vec![10.0, 15.0],
                cores: 2,
            },
            mem: MemInfo {
                used: 100,
                total: 200,
            },
            gpus: vec![GpuInfo {
                index: 0,
                name: "RTX".into(),
                util: 3.0,
                mem_used: 10,
                mem_total: 20,
                temp: Some(40.0),
            }],
            processes: vec![ProcessInfo {
                pid: 9,
                name: "p".into(),
                cpu: 1.0,
                mem: 2,
            }],
            error: None,
        };
        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"perCore\""));
        assert!(json.contains("\"memUsed\""));
        assert!(json.contains("\"memTotal\""));
        assert!(json.contains("\"kind\":\"local\""));
    }

    #[test]
    fn tray_summary_format_and_length_limit() {
        // 四舍五入到整数百分比
        assert_eq!(
            tray_summary(12.4, 74.2, 2, 3),
            "本机 CPU 12% · 内存 74% · 在线 2/3"
        );
        // 越界/非法数值被收敛，不出现 NaN
        assert_eq!(
            tray_summary(150.0, f64::NAN, 0, 0),
            "本机 CPU 100% · 内存 0% · 在线 0/1"
        );
        // 128 字符上限按字符数截断（Windows tooltip 限制）
        let long = "长".repeat(200);
        let cut = truncate_chars(&long, 128);
        assert_eq!(cut.chars().count(), 128);
        assert!(cut.ends_with('…'));
        assert!(tray_summary(1.0, 2.0, 1, 1).chars().count() <= 128);
    }
}
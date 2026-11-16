//! 配置：默认值、JSON 合并与落盘（app config 目录下的 config.json）。
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// 远端主机配置：id 为 `ssh:<alias>`，label 为显示名。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub id: String,
    pub label: String,
    pub alias: String,
    pub enabled: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            label: String::new(),
            alias: String::new(),
            enabled: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub refresh_ms: u64,
    pub history_points: usize,
    pub top_n: usize,
    pub opacity: f64,
    pub always_on_top: bool,
    /// 开机自启的期望状态（真实状态以注册表为准，启动时同步）
    pub autostart: bool,
    pub view: String,
    /// tile 贴片当前展示的主机 id（"local" 或 `ssh:<alias>`）
    pub tile_host_id: String,
    pub local_label: String,
    pub servers: Vec<ServerConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            refresh_ms: 2000,
            history_points: 180,
            top_n: 10,
            opacity: 0.92,
            always_on_top: true,
            autostart: false,
            view: "tile".to_string(),
            tile_host_id: "local".to_string(),
            local_label: "本机".to_string(),
            servers: Vec::new(),
        }
    }
}

impl AppConfig {
    /// 把非法值收敛到可用区间，避免坏配置让后台线程空转或窗口消失。
    pub fn sanitize(&mut self) {
        self.refresh_ms = self.refresh_ms.clamp(500, 60_000);
        self.history_points = self.history_points.clamp(10, 3600);
        self.top_n = self.top_n.clamp(1, 50);
        if !self.opacity.is_finite() {
            self.opacity = 0.92;
        }
        // 设置面板的可调区间：0.6–1.0（再低会让亮色壁纸下的文字不可读）
        self.opacity = self.opacity.clamp(0.6, 1.0);
        if !matches!(self.view.as_str(), "tile" | "detail" | "grid") {
            self.view = "tile".to_string();
        }
        if self.local_label.trim().is_empty() {
            self.local_label = "本机".to_string();
        }
        // 远端主机：补 id/label、丢弃无别名或重复项，避免后台线程空转。
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        self.servers.retain_mut(|server| {
            server.alias = server.alias.trim().to_string();
            if server.alias.is_empty() {
                return false;
            }
            if server.id.trim().is_empty() {
                server.id = server_id(&server.alias);
            }
            if server.label.trim().is_empty() {
                server.label = server.alias.clone();
            }
            seen.insert(server.id.clone())
        });
        if self.servers.len() > 32 {
            self.servers.truncate(32);
        }
        // tile 选中主机被删除/禁用后回落到本机。
        self.tile_host_id = self.tile_host_id.trim().to_string();
        if self.tile_host_id.is_empty() {
            self.tile_host_id = "local".to_string();
        }
        if self.tile_host_id != "local"
            && !self
                .servers
                .iter()
                .any(|server| server.enabled && server.id == self.tile_host_id)
        {
            self.tile_host_id = "local".to_string();
        }
    }

    /// 浅合并：patch 中的对象字段覆盖当前值，未给出字段保持原值。
    pub fn merged(&self, patch: &Value) -> Self {
        let mut base = serde_json::to_value(self).unwrap_or(Value::Null);
        if let (Some(base_obj), Some(patch_obj)) = (base.as_object_mut(), patch.as_object()) {
            for (key, value) in patch_obj {
                base_obj.insert(key.clone(), value.clone());
            }
        }
        let mut next: AppConfig =
            serde_json::from_value(base).unwrap_or_else(|_| AppConfig::default());
        next.sanitize();
        next
    }
}

/// 远端主机 id 约定：`ssh:<alias>`。
pub fn server_id(alias: &str) -> String {
    format!("ssh:{}", alias.trim())
}

/// 把 `~/.ssh/config` 的别名合并进 servers：只追加缺失项，返回新增的 alias。
///
/// 约定（与界面/README 一致）：
/// - 已存在的别名原样保留：label、enabled、排序都不动，不删除任何条目；
/// - 新条目 `id = ssh:<alias>`、`label = alias`、`enabled = true`；
/// - 按 alias / id 去重，重复调用不会产生重复项；
/// - 通配项（`*` `?` `!`）与空别名直接跳过。
pub fn merge_servers(existing: &mut Vec<ServerConfig>, aliases: &[String]) -> Vec<String> {
    let mut seen_ids: std::collections::HashSet<String> =
        existing.iter().map(|server| server.id.clone()).collect();
    let mut seen_aliases: std::collections::HashSet<String> = existing
        .iter()
        .map(|server| server.alias.trim().to_string())
        .collect();
    let mut added: Vec<String> = Vec::new();
    for raw in aliases {
        let alias = raw.trim();
        if alias.is_empty() || alias.contains(['*', '?', '!']) {
            continue;
        }
        let id = server_id(alias);
        if seen_ids.contains(&id) || seen_aliases.contains(alias) {
            continue;
        }
        seen_ids.insert(id.clone());
        seen_aliases.insert(alias.to_string());
        existing.push(ServerConfig {
            id,
            label: alias.to_string(),
            alias: alias.to_string(),
            enabled: true,
        });
        added.push(alias.to_string());
    }
    added
}

pub fn config_path(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

/// 读取配置；文件缺失或损坏时返回默认值（不 panic）。
pub fn load(path: &Path) -> AppConfig {
    let mut cfg = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<AppConfig>(&text).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    };
    cfg.sanitize();
    cfg
}

/// 写盘失败只返回错误信息，不 panic。
pub fn save(path: &Path, cfg: &AppConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化配置失败: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("写入配置失败: {e}"))
}

/// 首次运行写入默认配置；已有文件则保留原内容。
pub fn ensure_exists(path: &Path) -> AppConfig {
    if path.exists() {
        load(path)
    } else {
        let cfg = AppConfig::default();
        let _ = save(path, &cfg);
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_defaults_match_spec() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.refresh_ms, 2000);
        assert_eq!(cfg.history_points, 180);
        assert_eq!(cfg.top_n, 10);
        assert!((cfg.opacity - 0.92).abs() < f64::EPSILON);
        assert!(cfg.always_on_top);
        assert!(!cfg.autostart);
        assert_eq!(cfg.view, "tile");
        assert_eq!(cfg.tile_host_id, "local");
        assert_eq!(cfg.local_label, "本机");
        assert!(cfg.servers.is_empty());
        // 序列化字段为 camelCase
        let value = serde_json::to_value(&cfg).unwrap();
        assert!(value.get("refreshMs").is_some());
        assert!(value.get("alwaysOnTop").is_some());
        assert!(value.get("historyPoints").is_some());
        assert!(value.get("tileHostId").is_some());
        assert!(value.get("autostart").is_some());
    }

    #[test]
    fn config_merge_tile_host_opacity_and_autostart() {
        let cfg = AppConfig::default();
        let servers = json!([
            {"id": "ssh:ubun_local", "label": "ubun_local", "alias": "ubun_local", "enabled": true},
            {"id": "ssh:ubun", "label": "公网机", "alias": "ubun", "enabled": false}
        ]);
        // tileHostId 指向已启用的远程主机时保留
        let merged = cfg.merged(&json!({
            "tileHostId": "ssh:ubun_local",
            "opacity": 0.75,
            "autostart": true,
            "servers": servers
        }));
        assert_eq!(merged.tile_host_id, "ssh:ubun_local");
        assert!((merged.opacity - 0.75).abs() < f64::EPSILON);
        assert!(merged.autostart);

        // 不透明度低于 0.6 会被收敛（面板在亮色壁纸下仍需可读）
        let low = cfg.merged(&json!({"opacity": 0.1}));
        assert!((low.opacity - 0.6).abs() < f64::EPSILON);

        // 指向不存在/已禁用主机时回落到本机
        let with_servers = cfg.merged(&json!({"servers": servers.clone()}));
        let disabled = with_servers.merged(&json!({"tileHostId": "ssh:ubun"}));
        assert_eq!(disabled.tile_host_id, "local");
        let missing = with_servers.merged(&json!({"tileHostId": "ssh:ghost"}));
        assert_eq!(missing.tile_host_id, "local");
        // 清空后也回落到本机
        let empty = with_servers.merged(&json!({"tileHostId": "   "}));
        assert_eq!(empty.tile_host_id, "local");
    }

    #[test]
    fn config_backward_compatible_with_old_file() {
        // 0.1.0 的配置文件没有 tileHostId / autostart 字段
        let mut cfg: AppConfig = serde_json::from_str(
            r#"{
                "refreshMs": 3000,
                "historyPoints": 120,
                "topN": 8,
                "opacity": 0.9,
                "alwaysOnTop": false,
                "view": "grid",
                "localLabel": "工作站",
                "servers": [{"id": "ssh:ubun_local", "label": "ubun_local", "alias": "ubun_local", "enabled": true}]
            }"#,
        )
        .unwrap();
        cfg.sanitize();
        assert_eq!(cfg.refresh_ms, 3000);
        assert_eq!(cfg.view, "grid");
        assert_eq!(cfg.tile_host_id, "local");
        assert!(!cfg.autostart);
        assert_eq!(cfg.servers.len(), 1);

        // 完全不完整的配置：所有字段回退默认值，不 panic
        let mut bare: AppConfig = serde_json::from_str("{}").unwrap();
        bare.sanitize();
        assert_eq!(bare, AppConfig::default());
    }

    #[test]
    fn config_merge_keeps_untouched_fields_and_sanitizes() {
        let cfg = AppConfig::default();
        let merged = cfg.merged(&json!({"refreshMs": 1000, "view": "detail"}));
        assert_eq!(merged.refresh_ms, 1000);
        assert_eq!(merged.view, "detail");
        // 未在 patch 中出现的字段保持默认
        assert_eq!(merged.top_n, 10);
        assert_eq!(merged.local_label, "本机");
        assert_eq!(merged.tile_host_id, "local");

        // 越界值收敛
        let clamped = cfg.merged(&json!({"refreshMs": 10, "topN": 999, "opacity": 5.0, "view": "bogus"}));
        assert_eq!(clamped.refresh_ms, 500);
        assert_eq!(clamped.top_n, 50);
        assert!((clamped.opacity - 1.0).abs() < f64::EPSILON);
        assert_eq!(clamped.view, "tile");

        // 未知字段被忽略
        let extra = cfg.merged(&json!({"unknownField": true, "localLabel": "工作站"}));
        assert_eq!(extra.local_label, "工作站");
    }

    #[test]
    fn config_partial_file_falls_back_to_defaults() {
        let mut cfg: AppConfig = serde_json::from_str(r#"{"topN": 5}"#).unwrap();
        cfg.sanitize();
        assert_eq!(cfg.top_n, 5);
        assert_eq!(cfg.refresh_ms, 2000);
        assert_eq!(cfg.history_points, 180);
        assert_eq!(cfg.tile_host_id, "local");
        assert!(!cfg.autostart);
    }

    #[test]
    fn merge_servers_adds_missing_and_keeps_existing_state() {
        let mut servers = vec![
            ServerConfig {
                id: "ssh:ubun_local".into(),
                label: "ubuntu 工作站".into(),
                alias: "ubun_local".into(),
                enabled: true,
            },
            ServerConfig {
                id: "ssh:ubun".into(),
                label: "公网机".into(),
                alias: "ubun".into(),
                enabled: false, // 用户手动停用：同步后必须保留
            },
        ];
        let aliases = vec![
            "robot".to_string(),
            "ubun".to_string(),
            "ubun_local".to_string(),
        ];
        let added = merge_servers(&mut servers, &aliases);
        assert_eq!(added, vec!["robot".to_string()]);
        assert_eq!(servers.len(), 3);
        // 已有项的顺序 / 自定义 label / 停用状态都不变
        assert_eq!(servers[0].id, "ssh:ubun_local");
        assert_eq!(servers[0].label, "ubuntu 工作站");
        assert!(servers[0].enabled);
        assert_eq!(servers[1].label, "公网机");
        assert!(!servers[1].enabled);
        // 新条目：label=alias、默认启用
        assert_eq!(servers[2].id, "ssh:robot");
        assert_eq!(servers[2].alias, "robot");
        assert_eq!(servers[2].label, "robot");
        assert!(servers[2].enabled);

        // 幂等：再同步一次不会新增、不会重复
        let again = merge_servers(&mut servers, &aliases);
        assert!(again.is_empty());
        assert_eq!(servers.len(), 3);
        let unique: std::collections::HashSet<_> = servers.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn merge_servers_skips_wildcards_and_duplicate_aliases() {
        let mut servers: Vec<ServerConfig> = Vec::new();
        let aliases = vec![
            "robot".to_string(),
            "robot".to_string(),
            "*".to_string(),
            "web-?".to_string(),
            "!bad".to_string(),
            "  ".to_string(),
            "gpu01".to_string(),
        ];
        let added = merge_servers(&mut servers, &aliases);
        assert_eq!(added, vec!["robot".to_string(), "gpu01".to_string()]);
        assert_eq!(servers.len(), 2);
        // 新条目顺序跟随 ssh config
        assert_eq!(servers[0].id, "ssh:robot");
        assert_eq!(servers[1].id, "ssh:gpu01");
    }

    #[test]
    fn config_sanitizes_servers() {
        let cfg = AppConfig::default();
        let merged = cfg.merged(&json!({
            "servers": [
                {"id": "", "label": "", "alias": "ubun_local", "enabled": true},
                {"id": "ssh:ubun", "label": "公网机", "alias": "ubun", "enabled": false},
                {"id": "ssh:ubun", "label": "重复", "alias": "ubun", "enabled": true},
                {"id": "ssh:x", "label": "空别名", "alias": "  ", "enabled": true}
            ]
        }));
        assert_eq!(merged.servers.len(), 2);
        assert_eq!(merged.servers[0].id, "ssh:ubun_local");
        assert_eq!(merged.servers[0].label, "ubun_local");
        assert!(merged.servers[0].enabled);
        assert_eq!(merged.servers[1].label, "公网机");
        assert_eq!(server_id("robot"), "ssh:robot");
    }
}
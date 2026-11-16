//! Tauri 命令：前端只通过这些命令与后端/窗口交互。
use std::path::Path;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::collector::build_history;
use crate::config::{self, AppConfig, ServerConfig};
use crate::model::{HistoryPayload, HostRuntime, Snapshot};
use crate::state::{view_size, AppState};

#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> Option<Snapshot> {
    state
        .inner
        .lock()
        .ok()
        .and_then(|inner| inner.snapshot.clone())
}

#[tauri::command]
pub fn get_history(state: State<'_, AppState>) -> HistoryPayload {
    match state.inner.lock() {
        Ok(inner) => build_history(&inner.history),
        Err(_) => HistoryPayload::default(),
    }
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.config()
}

/// 合并 patch、落盘并返回新配置。
#[tauri::command]
pub fn set_config(
    app: AppHandle,
    state: State<'_, AppState>,
    patch: Value,
) -> Result<AppConfig, String> {
    let next = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "配置锁不可用".to_string())?;
        let next = inner.config.merged(&patch);
        inner.config = next.clone();
        next
    };
    config::save(&state.config_path, &next)?;
    let _ = crate::emit_remotes(&app);
    let _ = app.emit("sysmon://config", &next);

    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_always_on_top(next.always_on_top);
    }
    Ok(next)
}

/// `sync_ssh_servers` 的返回值：新增的别名列表与同步后的主机总数。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub added: Vec<String>,
    pub total: usize,
}

/// 托盘「重新加载配置」：从 config.json 重新读取并立即应用（不重启进程）。
pub fn reload_from_disk(app: &AppHandle) -> Result<AppConfig, String> {
    let state = app.state::<AppState>();
    let mut cfg = config::load(&state.config_path);
    // 重新加载时同样同步 ~/.ssh/config：只追加缺失别名，不动已有项。
    let added = sync_aliases_into(&mut cfg);
    if !added.is_empty() {
        config::save(&state.config_path, &cfg)?;
    }
    {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "配置锁不可用".to_string())?;
        inner.config = cfg.clone();
    }

    if let Some(win) = app.get_webview_window("main") {
        let (w, h) = view_size(&cfg.view);
        apply_window_size(&win, w, h)?;
        let _ = win.set_always_on_top(cfg.always_on_top);
    }
    if let Some(tray_state) = app.try_state::<crate::TrayState>() {
        let _ = tray_state.always_on_top.set_checked(cfg.always_on_top);
    }

    crate::emit_remotes(app);
    let _ = app.emit("sysmon://config", &cfg);
    Ok(cfg)
}

/// 读取 `~/.ssh/config` 的 Host 别名并合并进配置（纯文件读取，不等 `ssh -G`）。
/// 启动、托盘重载、设置面板手动同步都走这里。
pub fn sync_aliases_into(cfg: &mut AppConfig) -> Vec<String> {
    let aliases = crate::ssh::ssh_config_path()
        .map(|path| crate::ssh::read_ssh_aliases(&path))
        .unwrap_or_default();
    let added = config::merge_servers(&mut cfg.servers, &aliases);
    if !added.is_empty() {
        cfg.sanitize();
        println!("[sysmon] 从 ~/.ssh/config 同步新增主机: {}", added.join(", "));
    }
    added
}

/// 设置面板「同步 SSH 别名」：把 ssh config 中缺失的主机追加进配置并落盘。
#[tauri::command]
pub fn sync_ssh_servers(app: AppHandle, state: State<'_, AppState>) -> Result<SyncResult, String> {
    let (added, next, total) = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "配置锁不可用".to_string())?;
        let added = sync_aliases_into(&mut inner.config);
        let next = inner.config.clone();
        let total = next.servers.len();
        (added, next, total)
    };
    if !added.is_empty() {
        config::save(&state.config_path, &next)?;
        crate::emit_remotes(&app);
        let _ = app.emit("sysmon://config", &next);
    }
    Ok(SyncResult { added, total })
}

#[tauri::command]
pub fn reload_config(app: AppHandle) -> Result<AppConfig, String> {
    reload_from_disk(&app)
}

/// 读取真实开机自启状态（以注册表为准）。
#[tauri::command]
pub fn get_autostart() -> bool {
    crate::autostart::is_enabled()
}

/// 托盘 tooltip 的当前摘要文本（与托盘线程使用同一实现，供界面核对）。
#[tauri::command]
pub fn get_tray_summary(state: State<'_, AppState>) -> String {
    state.tray_text()
}

/// 写/删注册表中的自启项，回读校验后同步 config.json。
#[tauri::command]
pub fn set_autostart(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<bool, String> {
    let actual = crate::autostart::set_enabled(enabled)?;
    let next = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "配置锁不可用".to_string())?;
        inner.config.autostart = actual;
        inner.config.clone()
    };
    config::save(&state.config_path, &next)?;
    let _ = app.emit("sysmon://config", &next);
    Ok(actual)
}

/// 切换视图：由后端调整窗口尺寸/位置并持久化。
#[tauri::command]
pub fn set_view(
    app: AppHandle,
    state: State<'_, AppState>,
    view: String,
) -> Result<AppConfig, String> {
    let view = match view.as_str() {
        "tile" | "detail" | "grid" => view,
        _ => "tile".to_string(),
    };
    let (w, h) = view_size(&view);
    if let Some(win) = app.get_webview_window("main") {
        apply_window_size(&win, w, h)?;
        let _ = win.set_focus();
    }

    let next = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "配置锁不可用".to_string())?;
        inner.config.view = view;
        inner.config.clone()
    };
    config::save(&state.config_path, &next)?;
    Ok(next)
}

/// 保持左上角位置不变；超出屏幕时拉回可视区域。
fn apply_window_size(win: &WebviewWindow, w: f64, h: f64) -> Result<(), String> {
    let pos = win.outer_position().ok();
    win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(w, h)))
        .map_err(|e| format!("调整窗口尺寸失败: {e}"))?;

    let Some(pos) = pos else { return Ok(()) };
    let scale = win.scale_factor().unwrap_or(1.0);
    let monitor = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten());

    let (mut x, mut y) = (pos.x, pos.y);
    if let Some(monitor) = monitor {
        let mp = monitor.position();
        let ms = monitor.size();
        let win_w = (w * scale).round() as i32;
        let win_h = (h * scale).round() as i32;
        let max_x = (mp.x + ms.width as i32 - win_w).max(mp.x);
        let max_y = (mp.y + ms.height as i32 - win_h).max(mp.y);
        x = x.clamp(mp.x, max_x);
        y = y.clamp(mp.y, max_y);
    }
    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
    Ok(())
}

#[tauri::command]
pub fn toggle_always_on_top(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let cfg = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "配置锁不可用".to_string())?;
        inner.config.always_on_top = !inner.config.always_on_top;
        inner.config.clone()
    };
    if let Some(win) = app.get_webview_window("main") {
        win.set_always_on_top(cfg.always_on_top)
            .map_err(|e| format!("设置置顶失败: {e}"))?;
    }
    config::save(&state.config_path, &cfg)?;

    // 同步托盘勾选项
    if let Some(tray_state) = app.try_state::<crate::TrayState>() {
        let _ = tray_state.always_on_top.set_checked(cfg.always_on_top);
    }
    Ok(cfg.always_on_top)
}

/// 汇总 ~/.ssh/config 别名（已排除通配项）。放到阻塞线程池，避免 ssh -G 拖慢 UI。
#[tauri::command]
pub async fn list_ssh_aliases() -> Vec<crate::ssh::SshAlias> {
    tauri::async_runtime::spawn_blocking(crate::ssh::list_aliases)
        .await
        .unwrap_or_default()
}

/// 测试连接：跑一次完整采集，返回延迟或中文错误。
#[tauri::command]
pub async fn test_ssh_alias(alias: String) -> Result<crate::ssh::TestResult, String> {
    let alias = alias.trim().to_string();
    if alias.is_empty() {
        return Err("别名为空".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || crate::ssh::test_alias(&alias))
        .await
        .map_err(|err| format!("测试连接失败: {err}"))
}

#[tauri::command]
pub fn get_remote(state: State<'_, AppState>) -> Vec<HostRuntime> {
    state.remotes()
}

#[tauri::command]
pub fn get_remote_history(state: State<'_, AppState>, host_id: String) -> HistoryPayload {
    state.remote_history(&host_id)
}

/// 修改 servers 并落盘，返回新配置。
fn mutate_servers(
    state: &State<'_, AppState>,
    change: impl FnOnce(&mut Vec<ServerConfig>) -> Result<(), String>,
) -> Result<AppConfig, String> {
    let next = {
        let mut inner = state
            .inner
            .lock()
            .map_err(|_| "配置锁不可用".to_string())?;
        change(&mut inner.config.servers)?;
        inner.config.sanitize();
        inner.config.clone()
    };
    config::save(&state.config_path, &next)?;
    Ok(next)
}

#[tauri::command]
pub fn add_server(
    app: AppHandle,
    state: State<'_, AppState>,
    alias: String,
    label: Option<String>,
) -> Result<AppConfig, String> {
    let alias = alias.trim().to_string();
    if alias.is_empty() {
        return Err("请选择一个 SSH 别名".to_string());
    }
    let id = config::server_id(&alias);
    let label = label.unwrap_or_default().trim().to_string();
    let next = mutate_servers(&state, |servers| {
        if servers.iter().any(|server| server.id == id) {
            return Err("该主机已在列表中".to_string());
        }
        servers.push(ServerConfig {
            id: id.clone(),
            label: if label.is_empty() {
                alias.clone()
            } else {
                label.clone()
            },
            alias: alias.clone(),
            enabled: true,
        });
        Ok(())
    })?;
    crate::emit_remotes(&app);
    Ok(next)
}

#[tauri::command]
pub fn remove_server(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<AppConfig, String> {
    let next = mutate_servers(&state, |servers| {
        let before = servers.len();
        servers.retain(|server| server.id != id);
        if servers.len() == before {
            return Err("主机不存在".to_string());
        }
        Ok(())
    })?;
    crate::emit_remotes(&app);
    Ok(next)
}

#[tauri::command]
pub fn set_server_enabled(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<AppConfig, String> {
    let next = mutate_servers(&state, |servers| {
        let Some(server) = servers.iter_mut().find(|server| server.id == id) else {
            return Err("主机不存在".to_string());
        };
        server.enabled = enabled;
        Ok(())
    })?;
    crate::emit_remotes(&app);
    Ok(next)
}

#[tauri::command]
pub fn update_server(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    label: String,
) -> Result<AppConfig, String> {
    let label = label.trim().to_string();
    if label.is_empty() {
        return Err("显示名不能为空".to_string());
    }
    let next = mutate_servers(&state, |servers| {
        let Some(server) = servers.iter_mut().find(|server| server.id == id) else {
            return Err("主机不存在".to_string());
        };
        server.label = label.clone();
        Ok(())
    })?;
    crate::emit_remotes(&app);
    Ok(next)
}

/// 立刻触发一次采集（不等下一个周期）。
#[tauri::command]
pub fn refresh_remote(
    state: State<'_, AppState>,
    host_id: String,
) -> Result<(), String> {
    if host_id == "local" {
        return Ok(());
    }
    let known = state
        .config()
        .servers
        .iter()
        .any(|server| server.id == host_id && server.enabled);
    if !known {
        return Err("主机不存在或未启用".to_string());
    }
    state.mark_refresh(&host_id);
    Ok(())
}

#[tauri::command]
pub fn open_config_file(state: State<'_, AppState>) -> Result<(), String> {
    let path = state.config_path.clone();
    if !path.exists() {
        config::save(&path, &state.config())?;
    }
    open_in_explorer(&path)
}

/// 用 explorer /select 定位配置文件（不引入 shell 插件）。
pub fn open_in_explorer(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开配置文件失败: {e}"))
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("当前平台未实现打开配置文件".to_string())
    }
}

#[tauri::command]
pub fn hide_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        win.hide().map_err(|e| format!("隐藏窗口失败: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
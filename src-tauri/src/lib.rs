//! 系统监控贴片：Tauri 2 + React 19。
mod autostart;
mod collector;
mod commands;
mod config;
mod gpu_proc;
mod model;
mod ssh;
mod state;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

use collector::Collector;
use config::ServerConfig;
use model::{HostInfo, HOST_KIND_SSH};
use state::{label_of, view_size, AppState};

/// 托盘勾选项句柄，用于和配置状态同步。
pub struct TrayState {
    pub always_on_top: CheckMenuItem<Wry>,
}

fn toggle_show(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let visible = win.is_visible().unwrap_or(false);
        if visible {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

fn build_tray(app: &mut tauri::App, always_on_top: bool) -> tauri::Result<()> {
    let toggle_item = MenuItem::with_id(app, "toggle_show", "显示/隐藏", true, None::<&str>)?;
    let always_item =
        CheckMenuItem::with_id(app, "always_on_top", "置顶", true, always_on_top, None::<&str>)?;
    let reload_item = MenuItem::with_id(app, "reload_config", "重新加载配置", true, None::<&str>)?;
    let open_item = MenuItem::with_id(app, "open_config", "打开配置文件", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&toggle_item, &always_item, &reload_item, &open_item, &quit_item],
    )?;

    app.manage(TrayState {
        always_on_top: always_item,
    });

    let mut builder = TrayIconBuilder::with_id("sysmon-tray")
        .tooltip("系统监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle_show" => toggle_show(app),
            "always_on_top" => {
                let next = {
                    let state = app.state::<AppState>();
                    match state.inner.lock() {
                        Ok(mut inner) => {
                            inner.config.always_on_top = !inner.config.always_on_top;
                            Some(inner.config.clone())
                        }
                        Err(_) => None,
                    }
                };
                if let Some(cfg) = next {
                    if let Some(win) = app.get_webview_window("main") {
                        let _ = win.set_always_on_top(cfg.always_on_top);
                    }
                    if let Some(state) = app.try_state::<AppState>() {
                        let _ = config::save(&state.config_path, &cfg);
                    }
                    if let Some(tray_state) = app.try_state::<TrayState>() {
                        let _ = tray_state.always_on_top.set_checked(cfg.always_on_top);
                    }
                    let _ = app.emit("sysmon://config", &cfg);
                }
            }
            "reload_config" => {
                // 重新读取 config.json 并立即应用（servers/refreshMs/opacity/view/tileHostId）。
                let _ = commands::reload_from_disk(app);
            }
            "open_config" => {
                if let Some(state) = app.try_state::<AppState>() {
                    let path = state.config_path.clone();
                    if !path.exists() {
                        let _ = config::save(&path, &state.config());
                    }
                    let _ = commands::open_in_explorer(&path);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_show(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// 托盘 tooltip：每 5 秒刷新为「本机 CPU 12% · 内存 74% · 在线 2/3」。
/// m = 本机 + 已启用远程主机数，n = 当前在线数（含本机）。
fn spawn_tray_summary(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(5));
        let text = app.state::<AppState>().tray_text();
        if let Some(tray) = app.tray_by_id("sysmon-tray") {
            let _ = tray.set_tooltip(Some(text.as_str()));
        }
    });
}

/// 后台采集线程：按 refreshMs 采集并推给前端。
fn spawn_collector(app: AppHandle) {
    std::thread::spawn(move || {
        let initial = {
            let state = app.state::<AppState>();
            state.config()
        };

        let mut collector = Collector::new(&initial.local_label);
        let mut current_label = initial.local_label.clone();
        collector.prime();

        loop {
            let cfg = {
                let state = app.state::<AppState>();
                state.config()
            };
            if cfg.local_label != current_label {
                collector.set_label(&cfg.local_label);
                current_label = cfg.local_label.clone();
            }

            let snapshot = collector.collect(cfg.top_n.max(1));

            {
                let state = app.state::<AppState>();
                if let Ok(mut inner) = state.inner.lock() {
                    inner.history.push_back(snapshot.clone());
                    while inner.history.len() > cfg.history_points {
                        inner.history.pop_front();
                    }
                    inner.snapshot = Some(snapshot.clone());
                }
            }

            // 窗口隐藏时不推送事件（不再每 2 秒唤醒前端），重新显示后前端用
            // get_snapshot / get_history 补齐；隐藏期间也放慢采集节奏。
            let visible = app
                .get_webview_window("main")
                .and_then(|w| w.is_visible().ok())
                .unwrap_or(true);
            if visible {
                let _ = app.emit("sysmon://snapshot", &snapshot);
            }

            let interval = if visible {
                cfg.refresh_ms
            } else {
                cfg.refresh_ms.max(5000)
            };
            std::thread::sleep(Duration::from_millis(interval));
        }
    });
}

/// 远程主机线程句柄：用于配置变更时停止旧线程。
struct HostThread {
    stop: Arc<AtomicBool>,
    alias: String,
    label: String,
}

/// 把当前远程运行态推给前端（完整列表，前端整体替换）。
pub fn emit_remotes(app: &AppHandle) {
    let list = app.state::<AppState>().remotes();
    let _ = app.emit("sysmon://remote", &list);
}

/// 等待一个采集周期：可被 stop 或“立即刷新”打断，返回 false 表示线程应退出。
fn wait_cycle(app: &AppHandle, host_id: &str, stop: &AtomicBool, total_ms: u64) -> bool {
    let mut waited = 0u64;
    while waited < total_ms {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        if app.state::<AppState>().take_refresh(host_id) {
            return true;
        }
        let step = 150u64.min(total_ms - waited);
        std::thread::sleep(Duration::from_millis(step));
        waited += step;
    }
    !stop.load(Ordering::Relaxed)
}

/// 单台远程主机的工作线程：独立采样节奏 + 失败指数退避。
fn run_remote_worker(app: AppHandle, server: ServerConfig, stop: Arc<AtomicBool>) {
    let host = HostInfo {
        id: server.id.clone(),
        label: label_of(&server),
        kind: HOST_KIND_SSH.to_string(),
    };
    app.state::<AppState>().ensure_remote(host.clone());
    emit_remotes(&app);

    let mut failures = 0u32;
    let mut first = true;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let cfg = app.state::<AppState>().config();
        // 远程默认 3 秒起步，避免 SSH 往返过于频繁。
        let interval = cfg.refresh_ms.max(3000);
        if !first && !wait_cycle(&app, &host.id, &stop, interval) {
            return;
        }
        first = false;

        match ssh::collect_remote_with_retries(&server.alias, cfg.top_n.max(1), 3) {
            Ok((sample, latency_ms)) => {
                failures = 0;
                let snapshot = ssh::sample_to_snapshot(host.clone(), sample);
                app.state::<AppState>().apply_remote_success(
                    &host.id,
                    snapshot,
                    latency_ms,
                    cfg.history_points,
                );
                emit_remotes(&app);
            }
            Err(err) => {
                failures += 1;
                app.state::<AppState>().apply_remote_error(&host.id, host.clone(), err);
                emit_remotes(&app);
                let backoff = match failures {
                    1 => 5000,
                    2 => 10_000,
                    _ => 30_000,
                };
                if !wait_cycle(&app, &host.id, &stop, backoff) {
                    return;
                }
            }
        }
    }
}

/// 监督线程：按配置增删主机线程，配置变更（alias/label）时重建。
fn spawn_remote_supervisor(app: AppHandle) {
    std::thread::spawn(move || {
        let mut running: HashMap<String, HostThread> = HashMap::new();
        let mut signature = String::new();
        loop {
            let cfg = app.state::<AppState>().config();
            let enabled: Vec<ServerConfig> = cfg
                .servers
                .iter()
                .filter(|server| server.enabled)
                .cloned()
                .collect();

            let mut stale: Vec<String> = Vec::new();
            for (id, thread) in running.iter() {
                let keep = enabled
                    .iter()
                    .find(|server| &server.id == id)
                    .map(|server| server.alias == thread.alias && server.label == thread.label)
                    .unwrap_or(false);
                if !keep {
                    stale.push(id.clone());
                }
            }
            for id in stale {
                if let Some(thread) = running.remove(&id) {
                    thread.stop.store(true, Ordering::Relaxed);
                }
            }

            for server in enabled {
                if running.contains_key(&server.id) {
                    continue;
                }
                let stop = Arc::new(AtomicBool::new(false));
                let thread_stop = stop.clone();
                let worker_app = app.clone();
                let worker_server = server.clone();
                std::thread::spawn(move || run_remote_worker(worker_app, worker_server, thread_stop));
                running.insert(
                    server.id.clone(),
                    HostThread {
                        stop,
                        alias: server.alias,
                        label: server.label,
                    },
                );
            }

            app.state::<AppState>().prune_remotes();

            let next_signature = running
                .keys()
                .map(|id| {
                    let thread = &running[id];
                    format!("{id}|{}|{}", thread.alias, thread.label)
                })
                .collect::<Vec<_>>()
                .join(",");
            if next_signature != signature {
                signature = next_signature;
                emit_remotes(&app);
            }
            std::thread::sleep(Duration::from_millis(700));
        }
    });
}

/// `--debug-collect <alias>`：不启动界面，打印一次远程采集结果（供命令行验证）。
/// 设置 `SYSMON_DEBUG_LOG` 时同时追加到该日志文件（release exe 无控制台时也能留证据）。
pub fn debug_collect(alias: &str) -> i32 {
    let (ok, text) = ssh::debug_collect(alias);
    println!("{text}");
    if let Some(path) = std::env::var_os("SYSMON_DEBUG_LOG") {
        let path = std::path::PathBuf::from(path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let stamp = model::now_ms();
            let state = if ok { "OK" } else { "OFFLINE" };
            let _ = writeln!(file, "[{stamp}] {state} {text}");
        }
    }
    if ok {
        0
    } else {
        2
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let start_minimized = std::env::args().any(|arg| arg == "--minimized");
    tauri::Builder::default()
        // 单实例必须最先注册：第二个进程检测到已有实例后唤起窗口并立即退出，
        // 避免出现多套 SSH 轮询线程。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let handle = app.handle().clone();
            let config_dir = handle
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("sysmon-widget"));
            let path = config::config_path(&config_dir);
            let mut cfg = config::ensure_exists(&path);

            // 开机自启的真实状态以注册表为准（文件里的值只是缓存）。
            let actual_autostart = autostart::is_enabled();
            let autostart_changed = cfg.autostart != actual_autostart;
            if autostart_changed {
                cfg.autostart = actual_autostart;
            }

            // 启动时自动同步 ~/.ssh/config：把新增别名（如 robot）追加进 config.json，
            // 已有主机的 label / 启用状态 / 顺序保持不变。
            let added = commands::sync_aliases_into(&mut cfg);
            if autostart_changed || !added.is_empty() {
                let _ = config::save(&path, &cfg);
            }

            let (w, h) = view_size(&cfg.view);
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_always_on_top(cfg.always_on_top);
                let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(w, h)));
                if start_minimized {
                    let _ = win.hide();
                }
            }

            app.manage(AppState::new(cfg.clone(), path));
            build_tray(app, cfg.always_on_top)?;
            spawn_collector(handle.clone());
            spawn_remote_supervisor(handle.clone());
            spawn_tray_summary(handle.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关闭按钮只隐藏窗口，进程常驻托盘。
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::get_history,
            commands::get_config,
            commands::set_config,
            commands::set_view,
            commands::toggle_always_on_top,
            commands::open_config_file,
            commands::hide_window,
            commands::quit_app,
            commands::list_ssh_aliases,
            commands::test_ssh_alias,
            commands::get_remote,
            commands::get_remote_history,
            commands::add_server,
            commands::remove_server,
            commands::set_server_enabled,
            commands::update_server,
            commands::refresh_remote,
            commands::sync_ssh_servers,
            commands::reload_config,
            commands::get_autostart,
            commands::set_autostart,
            commands::get_tray_summary,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
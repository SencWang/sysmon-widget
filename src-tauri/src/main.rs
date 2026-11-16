// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
unsafe extern "system" {
    /// 让 release exe 在命令行里也能输出 `--debug-collect` 结果。
    fn AttachConsole(dw_process_id: u32) -> i32;
}

fn main() {
    // 命令行验证入口：--debug-collect <alias> 打印一次远程采集结果，不启动界面。
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--debug-collect" {
        #[cfg(windows)]
        unsafe {
            const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
        std::process::exit(sysmon_widget_lib::debug_collect(&args[2]));
    }
    sysmon_widget_lib::run()
}
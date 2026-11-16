//! 开机自启：写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 的 `SysmonWidget` 值。
//!
//! 只用当前用户注册表（不需要管理员权限），写入/删除后都**回读**真实状态，
//! 避免界面显示与系统实际状态不一致。
use std::path::Path;

/// 当前用户自启注册表键。
pub const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
/// 值名（与产品名一致，便于用户在 regedit 中识别）。
pub const VALUE_NAME: &str = "SysmonWidget";

/// 注册表值：`"<exe 绝对路径>"`（始终带引号，路径含空格也不会被拆开）。
pub fn autostart_command_line(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// `reg add` 参数（不含 `reg` 本身），便于单测校验引号与参数顺序。
pub fn add_args(exe: &Path) -> Vec<String> {
    vec![
        "add".to_string(),
        RUN_KEY.to_string(),
        "/v".to_string(),
        VALUE_NAME.to_string(),
        "/t".to_string(),
        "REG_SZ".to_string(),
        "/d".to_string(),
        autostart_command_line(exe),
        "/f".to_string(),
    ]
}

/// `reg delete` 参数（不含 `reg` 本身）。
pub fn delete_args() -> Vec<String> {
    vec![
        "delete".to_string(),
        RUN_KEY.to_string(),
        "/v".to_string(),
        VALUE_NAME.to_string(),
        "/f".to_string(),
    ]
}

/// `reg query` 参数（不含 `reg` 本身）。
pub fn query_args() -> Vec<String> {
    vec![
        "query".to_string(),
        RUN_KEY.to_string(),
        "/v".to_string(),
        VALUE_NAME.to_string(),
    ]
}

/// 解析 `reg query` 输出，返回该值的数据（去掉两侧空白）。
///
/// 输出形如：
/// ```text
/// HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run
///     SysmonWidget    REG_SZ    "D:\agent\sysmon-widget.exe"
/// ```
pub fn parse_query_output(stdout: &str) -> Option<String> {
    for raw in stdout.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with("HKEY") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        if !name.eq_ignore_ascii_case(VALUE_NAME) {
            continue;
        }
        // 第二列是类型（REG_SZ / REG_EXPAND_SZ），其余全部是数据（可能含空格）。
        let _kind = parts.next();
        let data = parts.collect::<Vec<_>>().join(" ");
        if data.is_empty() {
            return None;
        }
        return Some(data);
    }
    None
}

/// 读取真实自启状态；注册表不可用（非 Windows）时返回 false。
pub fn is_enabled() -> bool {
    #[cfg(windows)]
    {
        let mut cmd = crate::collector::hidden_command("reg");
        cmd.args(query_args());
        match cmd.output() {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                parse_query_output(&stdout).is_some()
            }
            _ => false,
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 当前 exe 的绝对路径（写入注册表用）。
pub fn exe_path() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()
}

/// 写入/删除自启项，然后回读校验，返回校验后的真实状态。
pub fn set_enabled(enabled: bool) -> Result<bool, String> {
    #[cfg(windows)]
    {
        let mut cmd = crate::collector::hidden_command("reg");
        if enabled {
            let exe = exe_path().ok_or_else(|| "无法获取 exe 路径".to_string())?;
            cmd.args(add_args(&exe));
        } else {
            cmd.args(delete_args());
        }
        let output = cmd
            .output()
            .map_err(|err| format!("调用 reg 失败: {err}"))?;

        let actual = is_enabled();
        if actual == enabled {
            return Ok(actual);
        }
        // 关闭时 reg delete 对“本就不存在”会返回非 0，此时以回读状态为准。
        if !enabled && !actual {
            return Ok(false);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("退出码 {:?}", output.status.code())
        } else {
            stderr
        };
        Err(format!("写入开机自启失败（回读状态不一致）：{detail}"))
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("当前平台未实现开机自启（仅支持 Windows）".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn autostart_value_wraps_exe_path_in_quotes() {
        // 路径含空格时引号必须保留
        let spaced = PathBuf::from(r"C:\Program Files\sysmon-widget\sysmon-widget.exe");
        assert_eq!(
            autostart_command_line(&spaced),
            r#""C:\Program Files\sysmon-widget\sysmon-widget.exe""#
        );
        let plain = PathBuf::from(r"D:\agent\sysmon-widget\src-tauri\target\release\sysmon-widget.exe");
        assert_eq!(
            autostart_command_line(&plain),
            r#""D:\agent\sysmon-widget\src-tauri\target\release\sysmon-widget.exe""#
        );
        // reg add 参数包含 /v SysmonWidget、REG_SZ 与带引号的路径
        let args = add_args(&spaced);
        assert_eq!(args[0], "add");
        assert!(args.iter().any(|a| a == RUN_KEY));
        assert!(args.iter().any(|a| a == VALUE_NAME));
        assert!(args.iter().any(|a| a == "REG_SZ"));
        assert!(args.last().is_some_and(|a| a == "/f"));
        assert!(args.iter().any(|a| a.starts_with('"')));
        // 删除参数只针对该值，不动整个 Run 键
        let del = delete_args();
        assert_eq!(del[0], "delete");
        assert_eq!(del.last().map(String::as_str), Some("/f"));
        assert!(query_args().iter().any(|a| a == VALUE_NAME));
    }

    #[test]
    fn parses_reg_query_output_and_missing_value() {
        let ok = "HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    SysmonWidget    REG_SZ    \"D:\\agent\\sysmon-widget\\sysmon-widget.exe\"\r\n";
        assert_eq!(
            parse_query_output(ok).as_deref(),
            Some(r#""D:\agent\sysmon-widget\sysmon-widget.exe""#)
        );
        // 仅键行（值不存在）与错误输出都返回 None
        assert_eq!(
            parse_query_output("HKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n"),
            None
        );
        assert_eq!(
            parse_query_output("ERROR: The system was unable to find the specified registry key or value."),
            None
        );
        assert_eq!(parse_query_output(""), None);
    }
}
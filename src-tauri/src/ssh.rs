//! SSH 远程采集：一次往返执行 POSIX sh 采集脚本，解析自描述输出。
//!
//! 设计要点：
//! - 脚本通过 stdin 传给 `ssh <alias> sh -s`，不写临时文件、不拼命令行字符串。
//! - 整体超时 8 秒（连接 5 秒 + 采集约 1.2 秒），超时直接 kill。
//! - 输出行以 `#` 开头自描述，Rust 侧解析；没有 `#END` 视为连接被截断。
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::collector::{hidden_command, parse_gpu_line};
use crate::gpu_proc::{merge_gpu_procs, GpuProc};
use crate::model::{GpuInfo, ProcessInfo, Snapshot};

/// 连接超时（秒），与脚本超时配合。
pub const CONNECT_TIMEOUT_SECS: u64 = 5;
/// 单次采集整体超时（毫秒）。
pub const COLLECT_TIMEOUT_MS: u64 = 8000;

/// 远程采集脚本：一次 ssh 往返拿到 CPU / 内存 / GPU / Top 进程。
pub const REMOTE_SCRIPT: &str = r##"#!/bin/sh
export LC_ALL=C
stat1=$(head -1 /proc/stat)
top_out=$(top -bn2 -d 1 -w 512 2>/dev/null)
stat2=$(head -1 /proc/stat)
memtotal=$(awk '/^MemTotal:/{print $2}' /proc/meminfo)
memavail=$(awk '/^MemAvailable:/{print $2}' /proc/meminfo)
echo "#CORES $(nproc 2>/dev/null || grep -c ^processor /proc/cpuinfo)"
echo "#LOAD $(cut -d' ' -f1-3 /proc/loadavg)"
echo "#STAT $stat1"
echo "#STAT $stat2"
echo "#MEM $memtotal $memavail"
if command -v nvidia-smi >/dev/null 2>&1; then
  nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits 2>/dev/null | sed 's/^/#GPU /'
  nvidia-smi pmon -c 1 2>/dev/null | grep -v '^#' | awk 'NF>3 {printf "#GPUPROC %s %s %s\n", $2, $4, $5}'
  nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader,nounits 2>/dev/null | sed 's/^/#GPUMEM /'
fi
echo "$top_out" | awk '
  /^[[:space:]]*PID[[:space:]]/ { n++; next }
  n == 2 && NF >= 12 { cmd=""; for(i=12;i<=NF;i++) cmd = cmd (i>12?" ":"") $i; printf "#PROC %s %s %s %s\n", $1, $9, $10, cmd }
'
echo "#END"
"##;

/// 远程一次采样的原始结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteSample {
    pub cores: usize,
    /// 0-100，`/proc/stat` 双采样计算
    pub cpu_usage: f64,
    pub mem_total: u64,
    pub mem_used: u64,
    pub gpus: Vec<GpuInfo>,
    pub processes: Vec<ProcessInfo>,
    /// 远端是否提供任意进程级 GPU 数据（pmon 或 compute-apps）
    pub gpu_proc_supported: bool,
}

/// ssh 配置里的一个别名（Host 行），hostname/user 由 `ssh -G` 补齐。
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshAlias {
    pub alias: String,
    pub hostname: String,
    pub user: String,
}

/// 测试连接结果。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub latency_ms: Option<u64>,
    pub message: String,
}

/// 构造 ssh 子进程（Windows 下隐藏控制台窗口）。
pub fn ssh_command(alias: &str) -> Command {
    let mut cmd = hidden_command("ssh");
    cmd.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg(format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"))
        .arg(alias)
        .arg("sh")
        .arg("-s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

fn read_stream<R: Read + Send + 'static>(stream: Option<R>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// 执行一次远程采集。任何失败都返回中文错误描述，不 panic。
pub fn collect_remote(alias: &str, top_n: usize) -> Result<(RemoteSample, u64), String> {
    let started = Instant::now();
    let mut child = ssh_command(alias)
        .spawn()
        .map_err(|e| format!("无法启动 ssh 客户端: {e}"))?;

    // 脚本写入 stdin 后立即关闭，避免 ssh 等待输入。
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(REMOTE_SCRIPT.as_bytes());
    }

    let out_handle = read_stream(child.stdout.take());
    let err_handle = read_stream(child.stderr.take());

    let deadline = started + Duration::from_millis(COLLECT_TIMEOUT_MS);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(_) => {
                let _ = child.kill();
                break None;
            }
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    let latency_ms = started.elapsed().as_millis() as u64;

    if timed_out {
        return Err(format!(
            "采集超时（超过 {} 秒），已中止本次连接",
            COLLECT_TIMEOUT_MS / 1000
        ));
    }
    let Some(status) = status else {
        return Err("ssh 进程异常退出".to_string());
    };
    if !status.success() {
        return Err(map_ssh_error(status.code(), &stderr));
    }

    let sample = parse_remote_output(&stdout, top_n)?;
    Ok((sample, latency_ms))
}

/// 远端偶发 `Connection closed`（sshd 限流/认证拒绝）时快速重试，避免把抖动当成离线。
pub fn collect_remote_with_retries(
    alias: &str,
    top_n: usize,
    attempts: usize,
) -> Result<(RemoteSample, u64), String> {
    let attempts = attempts.max(1);
    for index in 0..attempts {
        match collect_remote(alias, top_n) {
            Ok(result) => return Ok(result),
            Err(err) => {
                let transient = err.contains("连接被远端关闭");
                if !transient || index + 1 >= attempts {
                    return Err(err);
                }
                std::thread::sleep(Duration::from_millis(900));
            }
        }
    }
    Err("采集失败".to_string())
}

/// ssh 错误 -> 中文提示（含 stderr 前 300 字符）。
pub fn map_ssh_error(code: Option<i32>, stderr: &str) -> String {
    let low = stderr.to_lowercase();
    let detail = first_chars(stderr.trim(), 300);
    if code == Some(255) {
        if low.contains("permission denied") {
            return "需要密码或密钥认证（BatchMode 下无法交互登录）".to_string();
        }
        if low.contains("host key verification failed") {
            return "主机密钥校验失败（请先手动 ssh 登录并确认指纹）".to_string();
        }
        if low.contains("connection timed out") || low.contains("operation timed out") {
            return "连接超时（网络不可达或端口被屏蔽）".to_string();
        }
        if low.contains("could not resolve hostname") {
            return "域名解析失败（请检查 HostName）".to_string();
        }
        if low.contains("connection refused") {
            return "连接被拒绝（端口未开放）".to_string();
        }
        if low.contains("connection closed by") || low.contains("connection reset by") {
            return "连接被远端关闭（可能仅支持密码/密钥认证，BatchMode 无法交互登录）".to_string();
        }
        if detail.is_empty() {
            return "SSH 连接失败（退出码 255）".to_string();
        }
        return format!("SSH 连接失败：{detail}");
    }
    match detail.is_empty() {
        true => format!(
            "采集脚本失败（退出码 {}）",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "未知".to_string())
        ),
        false => format!(
            "采集脚本失败（退出码 {}）：{detail}",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "未知".to_string())
        ),
    }
}

fn first_chars(text: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().take(max) {
        out.push(ch);
    }
    out
}

/// 解析 `/proc/stat` 的 cpu 汇总行（双采样），返回 0-100 使用率。
pub fn cpu_usage_from_stats(stat1: &str, stat2: &str) -> f64 {
    let Some((idle1, total1)) = parse_cpu_line(stat1) else {
        return 0.0;
    };
    let Some((idle2, total2)) = parse_cpu_line(stat2) else {
        return 0.0;
    };
    let d_total = total2 as f64 - total1 as f64;
    if d_total <= 0.0 {
        return 0.0;
    }
    let d_idle = idle2 as f64 - idle1 as f64;
    (100.0 * (1.0 - d_idle / d_total)).clamp(0.0, 100.0)
}

/// 返回 (idle ticks, total ticks)。idle 取第 4 个字段（含 iowait 前）。
fn parse_cpu_line(line: &str) -> Option<(u64, u64)> {
    let mut parts = line.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("cpu") {
        return None;
    }
    let values: Vec<u64> = parts.filter_map(|p| p.parse::<u64>().ok()).collect();
    if values.len() < 4 {
        return None;
    }
    // 第 4 个字段（index 3）是 idle；guest/guest_nice 已包含在 user/nice 中，这里保守全加。
    let idle = values[3];
    let total: u64 = values.iter().sum();
    Some((idle, total))
}

/// 解析远程脚本输出；缺少 `#END` 视为采集失败。
pub fn parse_remote_output(stdout: &str, top_n: usize) -> Result<RemoteSample, String> {
    let mut cores = 0usize;
    let mut stats: Vec<&str> = Vec::new();
    let mut mem_total_kb = 0u64;
    let mut mem_avail_kb = 0u64;
    let mut gpus: Vec<GpuInfo> = Vec::new();
    let mut gpu_procs: Vec<GpuProc> = Vec::new();
    let mut gpu_apps: Vec<(u32, u64)> = Vec::new();
    let mut raw_procs: Vec<(u32, f64, f64, String)> = Vec::new();
    let mut ended = false;

    for raw in stdout.lines() {
        let line = raw.trim_end_matches('\r');
        if line == "#END" {
            ended = true;
            break;
        }
        if let Some(rest) = line.strip_prefix("#CORES ") {
            cores = rest.trim().parse().unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("#STAT ") {
            if stats.len() < 2 {
                stats.push(rest);
            }
        } else if let Some(rest) = line.strip_prefix("#MEM ") {
            let mut it = rest.split_whitespace();
            mem_total_kb = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            mem_avail_kb = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("#GPU ") {
            if let Some(gpu) = parse_gpu_line(rest) {
                gpus.push(gpu);
            }
        } else if let Some(rest) = line.strip_prefix("#GPUPROC ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }
            let Ok(pid) = parts[0].parse::<u32>() else {
                continue;
            };
            let metric = |text: &str| -> Option<f64> {
                if text == "-" {
                    None
                } else {
                    text.parse::<f64>().ok().map(|v| v.clamp(0.0, 100.0))
                }
            };
            gpu_procs.push(GpuProc {
                pid,
                gpu: 0,
                kind: String::new(),
                sm: metric(parts[1]),
                mem_pct: metric(parts[2]),
                command: String::new(),
            });
        } else if let Some(rest) = line.strip_prefix("#GPUMEM ") {
            let mut it = rest
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|value| !value.is_empty());
            let (Some(pid), Some(mib)) = (
                it.next().and_then(|v| v.trim().parse::<u32>().ok()),
                it.next().and_then(|v| v.trim().parse::<f64>().ok()),
            ) else {
                continue;
            };
            if mib.is_finite() && mib >= 0.0 {
                gpu_apps.push((pid, (mib * 1024.0 * 1024.0) as u64));
            }
        } else if let Some(rest) = line.strip_prefix("#PROC ") {
            let parts: Vec<&str> = rest.splitn(4, ' ').collect();
            if parts.len() < 4 {
                continue;
            }
            let (Ok(pid), Ok(cpu), Ok(mem_pct)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<f64>(),
                parts[2].parse::<f64>(),
            ) else {
                continue;
            };
            let name = parts[3].trim();
            if name.is_empty() {
                continue;
            }
            raw_procs.push((pid, cpu, mem_pct, name.to_string()));
        }
    }

    if !ended {
        return Err("采集输出不完整（连接被中断）".to_string());
    }

    let mem_total = mem_total_kb.saturating_mul(1024);
    let mem_used = mem_total_kb
        .saturating_sub(mem_avail_kb.min(mem_total_kb))
        .saturating_mul(1024);

    let processes: Vec<ProcessInfo> = raw_procs
        .into_iter()
        .map(|(pid, cpu, mem_pct, name)| ProcessInfo {
            pid,
            name,
            cpu: cpu.max(0.0),
            mem: (mem_pct.max(0.0) * mem_total_kb as f64 * 1024.0 / 100.0) as u64,
            gpu_util: None,
            gpu_mem: None,
        })
        .collect();
    let gpu_total_bytes: Vec<u64> = gpus.iter().map(|gpu| gpu.mem_total).collect();
    let mut merged = Snapshot {
        host: crate::model::HostInfo::local(""),
        ts: 0,
        cpu: crate::model::CpuInfo::default(),
        mem: crate::model::MemInfo::default(),
        gpus: Vec::new(),
        processes,
        error: None,
        gpu_proc_supported: false,
    };
    merge_gpu_procs(
        &mut merged,
        &gpu_procs,
        &gpu_apps,
        &gpu_total_bytes,
        top_n.max(1),
    );
    let processes = merged.processes;
    let gpu_proc_supported = merged.gpu_proc_supported;

    let cpu_usage = match (stats.first(), stats.get(1)) {
        (Some(a), Some(b)) => cpu_usage_from_stats(a, b),
        _ => 0.0,
    };

    Ok(RemoteSample {
        cores: if cores == 0 { 1 } else { cores },
        cpu_usage,
        mem_total,
        mem_used,
        gpus,
        processes,
        gpu_proc_supported,
    })
}

/// 由采样结果构造 Snapshot。
pub fn sample_to_snapshot(host: crate::model::HostInfo, sample: RemoteSample) -> Snapshot {
    Snapshot {
        host,
        ts: crate::model::now_ms(),
        cpu: crate::model::CpuInfo {
            usage: sample.cpu_usage,
            per_core: Vec::new(),
            cores: sample.cores,
        },
        mem: crate::model::MemInfo {
            used: sample.mem_used,
            total: sample.mem_total,
        },
        gpus: sample.gpus,
        processes: sample.processes,
        error: None,
        gpu_proc_supported: sample.gpu_proc_supported,
    }
}

/// 用户 ssh 配置路径：Windows 用 %USERPROFILE%，其他平台用 $HOME。
pub fn ssh_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".ssh").join("config"))
}

/// 读取并解析 ssh config；文件缺失返回空列表。
pub fn read_ssh_aliases(path: &std::path::Path) -> Vec<String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&bytes);
    parse_ssh_config(&text)
}

/// 解析 `Host` 行：忽略通配（* ? !）、注释与 Include，按别名排序去重。
pub fn parse_ssh_config(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, rest) = match line.split_once(|c: char| c == '=' || c.is_whitespace()) {
            Some((key, rest)) => (key, rest.trim_start_matches('=').trim()),
            None => continue,
        };
        if !key.eq_ignore_ascii_case("host") {
            continue;
        }
        for token in rest.split_whitespace() {
            if token.contains(['*', '?', '!']) {
                continue;
            }
            if !out.iter().any(|alias| alias == token) {
                out.push(token.to_string());
            }
        }
    }
    out.sort();
    out
}

/// 用 `ssh -G <alias>` 补齐 hostname / user；失败留空。
pub fn resolve_alias(alias: &str) -> SshAlias {
    let mut result = SshAlias {
        alias: alias.to_string(),
        hostname: String::new(),
        user: String::new(),
    };
    let output = match hidden_command("ssh")
        .arg("-G")
        .arg(alias)
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return result,
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        if key.eq_ignore_ascii_case("hostname") && result.hostname.is_empty() {
            result.hostname = value.trim().to_string();
        } else if key.eq_ignore_ascii_case("user") && result.user.is_empty() {
            result.user = value.trim().to_string();
        }
    }
    result
}

/// 列出 ssh config 中的别名（已排除通配），并按 alias 排序。
pub fn list_aliases() -> Vec<SshAlias> {
    let Some(path) = ssh_config_path() else {
        return Vec::new();
    };
    let aliases = read_ssh_aliases(&path);
    let mut list: Vec<SshAlias> = aliases.iter().map(|a| resolve_alias(a)).collect();
    list.sort_by(|a, b| a.alias.cmp(&b.alias));
    list
}

/// 测试连接：跑一次完整采集，返回延迟与摘要。
pub fn test_alias(alias: &str) -> TestResult {
    match collect_remote_with_retries(alias, 10, 2) {
        Ok((sample, latency_ms)) => TestResult {
            ok: true,
            latency_ms: Some(latency_ms),
            message: format!(
                "连接成功：{} 核 · {} 个 GPU · {} 条进程 · {} ms",
                sample.cores,
                sample.gpus.len(),
                sample.processes.len(),
                latency_ms
            ),
        },
        Err(err) => TestResult {
            ok: false,
            latency_ms: None,
            message: err,
        },
    }
}

/// 调试入口用：把一次采集结果格式化成多行文本。
pub fn debug_collect(alias: &str) -> (bool, String) {
    match collect_remote_with_retries(alias, 10, 3) {
        Ok((sample, latency_ms)) => {
            let mut lines = vec![
                format!(
                    "[{}] OK latency={}ms cores={} cpu={:.1}% mem={:.1}/{:.1} GiB gpus={} procs={}",
                    alias,
                    latency_ms,
                    sample.cores,
                    sample.cpu_usage,
                    sample.mem_used as f64 / 1024.0 / 1024.0 / 1024.0,
                    sample.mem_total as f64 / 1024.0 / 1024.0 / 1024.0,
                    sample.gpus.len(),
                    sample.processes.len()
                ),
            ];
            for gpu in &sample.gpus {
                lines.push(format!(
                    "  GPU #{} {} util={:.0}% mem={}/{} MiB temp={}",
                    gpu.index,
                    gpu.name,
                    gpu.util,
                    gpu.mem_used / 1024 / 1024,
                    gpu.mem_total / 1024 / 1024,
                    gpu.temp
                        .map(|t| format!("{t:.0}°C"))
                        .unwrap_or_else(|| "--".into())
                ));
            }
            for proc_ in &sample.processes {
                lines.push(format!(
                    "  PROC pid={} cpu={:.1}% mem={:.1} MiB gpu={} gpu_mem={} MiB name={}",
                    proc_.pid,
                    proc_.cpu,
                    proc_.mem as f64 / 1024.0 / 1024.0,
                    proc_.gpu_util
                        .map(|v| format!("{v:.0}%"))
                        .unwrap_or_else(|| "--".into()),
                    proc_.gpu_mem
                        .map(|v| format!("{:.0}", v as f64 / 1024.0 / 1024.0))
                        .unwrap_or_else(|| "--".into()),
                    proc_.name
                ));
            }
            lines.push(format!(
                "  GPU_PROC_SUPPORTED={}",
                sample.gpu_proc_supported
            ));
            (true, lines.join("\n"))
        }
        Err(err) => (false, format!("[{alias}] OFFLINE: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OK: &str = "\
#CORES 256
#LOAD 107.53 106.58 96.08
#STAT cpu  8192019136 40199128 237279678 53197795046 2564302 0 442576 0 0 0
#STAT cpu  8192027378 40199128 237281471 53197820430 2564304 0 442576 0 0 0
#MEM 263876660 239894008
#GPU 0, NVIDIA GeForce RTX 4090, 19, 9915, 24564, 51
#GPUPROC 1450352 24 2
#GPUPROC 7657 - -
#GPUMEM 1450352, 8066
#GPUMEM 999, 4096
#PROC 1450352 5877 3.2 python
#PROC 7657 25.0 0.4 gnome-shell
#PROC 1450351 22.2 0.5 中文进程名 带空格
#PROC broken 1 2 bad
#PROC 4688 9.3 0.3 ToDesk_Service
#END
";

    #[test]
    fn parses_remote_output_with_many_cores_and_chinese_names() {
        let sample = parse_remote_output(SAMPLE_OK, 10).unwrap();
        assert_eq!(sample.cores, 256);
        assert_eq!(sample.gpus.len(), 1);
        assert_eq!(sample.gpus[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(sample.mem_total, 263876660 * 1024);
        assert_eq!(sample.mem_used, (263876660 - 239894008) * 1024);
        // 坏行被跳过，中文进程名保留；top 的 %CPU 在多核机器上可远超 100
        assert_eq!(sample.processes.len(), 5);
        assert_eq!(sample.processes[0].name, "python");
        assert!((sample.processes[0].cpu - 5877.0).abs() < f64::EPSILON);
        // 进程级 GPU：pmon 的 SM% + compute-apps 的精确显存（MiB）
        assert!(sample.gpu_proc_supported);
        assert_eq!(sample.processes[0].gpu_util, Some(24.0));
        assert_eq!(sample.processes[0].gpu_mem, Some(8066 * 1024 * 1024));
        // GPU 显存大但 CPU 不在 Top N 的进程也会并入
        let gpu_only = sample.processes.iter().find(|p| p.pid == 999).unwrap();
        assert_eq!(gpu_only.cpu, 0.0);
        assert_eq!(gpu_only.gpu_mem, Some(4096 * 1024 * 1024));
        assert_eq!(sample.processes[1].name, "gnome-shell");
        assert!((sample.processes[1].cpu - 25.0).abs() < f64::EPSILON);
        assert!(sample.processes.iter().any(|p| p.name == "中文进程名 带空格"));
        // mem bytes = 3.2% * total
        let python = sample.processes.iter().find(|p| p.pid == 1450352).unwrap();
        let expect = (3.2 * 263876660.0 * 1024.0 / 100.0) as u64;
        assert_eq!(python.mem, expect);
        // CPU 使用率在 0-100 之间且不为 0（两次 stat 有差值）
        assert!(sample.cpu_usage > 0.0 && sample.cpu_usage <= 100.0);
    }

    #[test]
    fn parses_remote_output_without_gpu_or_end() {
        let no_gpu = "\
#CORES 8
#STAT cpu  100 0 100 800 0 0 0 0 0 0
#STAT cpu  120 0 110 850 0 0 0 0 0 0
#MEM 1000 500
#PROC 1 1.0 2.0 init
#END
";
        let sample = parse_remote_output(no_gpu, 10).unwrap();
        assert_eq!(sample.cores, 8);
        assert!(sample.gpus.is_empty());
        assert_eq!(sample.processes.len(), 1);
        assert_eq!(sample.processes[0].mem, 2 * 1000 * 1024 / 100);

        // 缺少 #END：连接被截断
        let truncated = no_gpu.replace("#END\n", "");
        assert!(parse_remote_output(&truncated, 10).is_err());

        // 全坏行也不 panic，返回空进程
        let broken = "#CORES x\n#MEM a b\n#GPU garbage\n#PROC 1 2\n#END\n";
        let sample = parse_remote_output(broken, 10).unwrap();
        assert!(sample.processes.is_empty());
        assert!(sample.gpus.is_empty());
        assert_eq!(sample.cpu_usage, 0.0);
    }

    #[test]
    fn cpu_usage_from_double_sampling() {
        // 总 tick 增加 1000，idle 增加 250 -> 75% 使用率
        let a = "cpu  100 0 100 500 0 0 0 0 0 0";
        let b = "cpu  600 0 350 750 0 0 0 0 0 0";
        assert!((cpu_usage_from_stats(a, b) - 75.0).abs() < 1e-6);
        // 无变化 -> 0，不除零
        assert_eq!(cpu_usage_from_stats(a, a), 0.0);
        // 坏行 -> 0
        assert_eq!(cpu_usage_from_stats("cpu", a), 0.0);
        assert_eq!(cpu_usage_from_stats("", ""), 0.0);
        // 完全空闲 -> 0
        let c = "cpu  0 0 0 1000 0 0 0 0 0 0";
        let d = "cpu  0 0 0 2000 0 0 0 0 0 0";
        assert_eq!(cpu_usage_from_stats(c, d), 0.0);
    }

    #[test]
    fn parses_ssh_config_host_aliases_and_excludes_wildcards() {
        let text = "\
# 注释
Host ubun_local
    HostName 172.26.202.49
    User ubuntu

Host ubun
    HostName 26.tcp.cpolar.top

Host=robot
    HostName 172.26.162.138

Host *.internal ?proxy !blocked bastion other
Include ~/.ssh/other-config
Match host foo
";
        let aliases = parse_ssh_config(text);
        assert_eq!(aliases, vec!["bastion", "other", "robot", "ubun", "ubun_local"]);
        assert!(!aliases.iter().any(|a| a.contains('*') || a.contains('?') || a.contains('!')));
        // 空文本安全
        assert!(parse_ssh_config("").is_empty());
    }

    #[test]
    fn maps_offline_errors_to_chinese_hints() {
        let password = map_ssh_error(
            Some(255),
            "ubuntu@26.tcp.cpolar.top: Permission denied (publickey,password).",
        );
        assert!(password.contains("需要密码"), "{password}");

        let host_key = map_ssh_error(Some(255), "Host key verification failed.");
        assert!(host_key.contains("主机密钥"), "{host_key}");

        let timeout = map_ssh_error(Some(255), "ssh: connect to host 1.2.3.4 port 22: Connection timed out");
        assert!(timeout.contains("连接超时"), "{timeout}");

        let dns = map_ssh_error(Some(255), "ssh: Could not resolve hostname nope: Name or service not known");
        assert!(dns.contains("解析"), "{dns}");

        let script = map_ssh_error(Some(1), "sh: top: not found");
        assert!(script.contains("采集脚本失败"), "{script}");
        assert!(script.contains("top: not found"), "{script}");

        // 远端直接断开连接（密码专用主机常见）：提示认证方式
        let closed = map_ssh_error(Some(255), "Connection closed by 1.2.3.4 port 22");
        assert!(closed.contains("密码") || closed.contains("认证"), "{closed}");
    }

    #[test]
    fn parses_multiple_gpus_and_maps_to_snapshot() {
        // nvidia-smi 一次输出全部 GPU：detail 需要列出 0..N，历史曲线只取 #0
        let multi = "\
#CORES 64
#STAT cpu  1000 0 500 8000 0 0 0 0 0 0
#STAT cpu  1200 0 600 8800 0 0 0 0 0 0
#MEM 32768000 16384000
#GPU 0, NVIDIA A100-SXM4-40GB, 10, 4096, 40960, 40
#GPU 1, NVIDIA A100-SXM4-40GB, 90, 20480, 40960, 61
#END
";
        let sample = parse_remote_output(multi, 10).unwrap();
        assert_eq!(sample.gpus.len(), 2);
        assert_eq!(sample.gpus[0].index, 0);
        assert_eq!(sample.gpus[1].index, 1);
        assert!((sample.gpus[0].util - 10.0).abs() < f64::EPSILON);
        assert!((sample.gpus[1].util - 90.0).abs() < f64::EPSILON);
        assert_eq!(sample.gpus[1].mem_used, 20480 * 1024 * 1024);

        let host = crate::model::HostInfo {
            id: "ssh:a100".into(),
            label: "a100".into(),
            kind: crate::model::HOST_KIND_SSH.into(),
        };
        let snapshot = sample_to_snapshot(host, sample);
        assert_eq!(snapshot.gpus.len(), 2);
        assert_eq!(snapshot.mem.total, 32768000 * 1024);

        // 历史曲线取 GPU #0（10% / 4096/40960 = 10%）
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(snapshot);
        let history = crate::collector::build_history(&queue);
        assert!((history.gpu[0] - 10.0).abs() < 1e-6);
        assert!((history.vram[0] - 10.0).abs() < 1e-6);
    }
}
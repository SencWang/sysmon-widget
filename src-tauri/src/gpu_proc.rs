//! 进程级 GPU 采集：`nvidia-smi pmon` + `--query-compute-apps`。
//!
//! 两种数据源互补：
//! - `pmon -c 1` 给出每个 pid 的 SM 利用率（%）与显存百分比（%），但显存百分比是相对值；
//! - `--query-compute-apps` 给出精确的 used_memory（MiB）。
//!
//! 驱动/环境不支持时（例如 Windows WDDM）两者都可能为空或直接失败，所有解析与
//! 合并函数都按“字段缺失”处理，不 panic、不记录日志。
use std::collections::{HashMap, HashSet};

use crate::collector::{hidden_command, nvidia_smi_candidates};
use crate::model::{ProcessInfo, Snapshot};

const MIB: u64 = 1024 * 1024;

/// `pmon` 中的一条进程记录（同一 pid 可能出现在多块 GPU 上）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuProc {
    pub pid: u32,
    pub gpu: u32,
    /// C / G / C+G
    pub kind: String,
    /// SM 利用率 0-100
    pub sm: Option<f64>,
    /// 显存占整卡百分比 0-100
    pub mem_pct: Option<f64>,
    /// pmon 输出的 command（可能被驱动截断，只用于补进程名）
    pub command: String,
}

fn parse_metric(text: &str) -> Option<f64> {
    let value = text.trim();
    if value.is_empty() || value == "-" {
        return None;
    }
    value.parse::<f64>().ok().map(|v| v.clamp(0.0, 100.0))
}

/// 解析 `nvidia-smi pmon -c 1`。
///
/// 表头行以 `#` 开头；列数随驱动变化（新版多出 jpg/ofa）。用第一行表头里的
/// `sm` / `mem` 定位指标列，缺失表头时退回固定的第 4、5 列（0-based 3、4）。
pub fn parse_pmon(stdout: &str) -> Vec<GpuProc> {
    let mut sm_idx = 3usize;
    let mut mem_idx = 4usize;
    let mut out = Vec::new();

    for raw in stdout.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            let tokens: Vec<&str> = line.trim_start_matches('#').split_whitespace().collect();
            if tokens
                .first()
                .map(|t| t.eq_ignore_ascii_case("gpu"))
                .unwrap_or(false)
            {
                if let Some(index) = tokens.iter().position(|t| t.eq_ignore_ascii_case("sm")) {
                    sm_idx = index;
                }
                if let Some(index) = tokens.iter().position(|t| t.eq_ignore_ascii_case("mem")) {
                    mem_idx = index;
                }
            }
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        let needed = (sm_idx.max(mem_idx) + 1).max(5);
        if parts.len() < needed {
            continue;
        }
        let (Ok(gpu), Ok(pid)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
            continue;
        };
        let kind = parts[2].to_string();
        if kind.is_empty() {
            continue;
        }
        // 指标列之后是 command；驱动可能截断，但至少保留最后一段。
        let command = parts
            .iter()
            .skip(mem_idx + 1)
            .last()
            .map(|value| value.to_string())
            .unwrap_or_default();
        out.push(GpuProc {
            pid,
            gpu,
            kind,
            sm: parse_metric(parts[sm_idx]),
            mem_pct: parse_metric(parts[mem_idx]),
            command,
        });
    }
    out
}

/// 解析 `--query-compute-apps=pid,process_name,used_memory` 的 CSV。
///
/// 返回 `(pid, bytes)`。路径中可能带空格或逗号，因此 pid 取第一个字段、
/// used_memory 取最后一个字段；中间的名字不参与解析。
pub fn parse_compute_apps(stdout: &str) -> Vec<(u32, u64)> {
    let mut out = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(pid_text) = line.split(',').next() else {
            continue;
        };
        let Some(mem_text) = line.rsplit(',').next() else {
            continue;
        };
        let (Ok(pid), Ok(mem_mib)) = (
            pid_text.trim().parse::<u32>(),
            mem_text.trim().parse::<f64>(),
        ) else {
            continue;
        };
        if !mem_mib.is_finite() || mem_mib < 0.0 {
            continue;
        }
        out.push((pid, (mem_mib * MIB as f64) as u64));
    }
    out
}

/// 同一 pid 的聚合结果（多卡时取较大的利用率/显存比例）。
#[derive(Default)]
struct Usage {
    sm: Option<f64>,
    mem_pct: Option<f64>,
    gpu: Option<u32>,
    command: String,
}

fn max_opt(current: Option<f64>, next: f64) -> Option<f64> {
    Some(current.map(|value| value.max(next)).unwrap_or(next))
}

fn pmon_bytes(usage: &Usage, gpu_total_bytes: &[u64]) -> Option<u64> {
    let pct = usage.mem_pct?;
    let total = usage
        .gpu
        .and_then(|index| gpu_total_bytes.get(index as usize).copied())
        .or_else(|| {
            // 只有一张卡时索引不匹配也可以安全回退。
            if gpu_total_bytes.len() == 1 {
                gpu_total_bytes.first().copied()
            } else {
                None
            }
        })?;
    Some((pct / 100.0 * total as f64) as u64)
}

fn sort_by_cpu_desc(processes: &mut [ProcessInfo]) {
    processes.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.mem.cmp(&a.mem))
    });
}

/// 把进程级 GPU 数据合并进快照。
///
/// 1. 已有进程按 pid 填 `gpu_util` / `gpu_mem`（显存优先用 compute-apps 的精确值）；
/// 2. 把按显存排名前 `top_n` 的 pid 补进进程表（CPU 占比通常很低，但仍值得展示）；
/// 3. 按 CPU 降序排序并截断到 `max(top_n, 10)`。
///
/// 任何字段缺失只影响对应字段，不影响 CPU/内存/GPU 汇总。
pub fn merge_gpu_procs(
    snapshot: &mut Snapshot,
    pmon: &[GpuProc],
    apps: &[(u32, u64)],
    gpu_total_bytes: &[u64],
    top_n: usize,
) {
    let mut by_pid: HashMap<u32, Usage> = HashMap::new();
    for item in pmon {
        let usage = by_pid.entry(item.pid).or_default();
        if let Some(sm) = item.sm {
            usage.sm = max_opt(usage.sm, sm);
        }
        if let Some(pct) = item.mem_pct {
            usage.mem_pct = max_opt(usage.mem_pct, pct);
        }
        if usage.gpu.is_none() {
            usage.gpu = Some(item.gpu);
        }
        if usage.command.is_empty() && !item.command.is_empty() {
            usage.command = item.command.clone();
        }
    }

    let mut app_mem: HashMap<u32, u64> = HashMap::new();
    for (pid, bytes) in apps {
        app_mem
            .entry(*pid)
            .and_modify(|current| *current = (*current).max(*bytes))
            .or_insert(*bytes);
    }

    for process in snapshot.processes.iter_mut() {
        if let Some(usage) = by_pid.get(&process.pid) {
            process.gpu_util = usage.sm;
            process.gpu_mem = app_mem
                .get(&process.pid)
                .copied()
                .or_else(|| pmon_bytes(usage, gpu_total_bytes));
        } else if let Some(bytes) = app_mem.get(&process.pid) {
            process.gpu_mem = Some(*bytes);
        }
    }

    // 候选集合 = 按显存排序的 GPU 进程（pmon 百分比在缺少 compute-apps 时作为估算）。
    let mut candidates: Vec<(u32, u64)> = app_mem
        .iter()
        .map(|(pid, bytes)| (*pid, *bytes))
        .collect();
    for (pid, usage) in by_pid.iter() {
        if app_mem.contains_key(pid) {
            continue;
        }
        if let Some(bytes) = pmon_bytes(usage, gpu_total_bytes) {
            if bytes > 0 {
                candidates.push((*pid, bytes));
            }
        }
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    candidates.truncate(top_n.max(1));
    // 保留 GPU 显存前 N 的 pid，用于截断时避免它们被 CPU 排序挤出表格。
    let gpu_top: HashSet<u32> = candidates.iter().map(|(pid, _)| *pid).collect();

    let existing: HashSet<u32> = snapshot.processes.iter().map(|p| p.pid).collect();
    for (pid, bytes) in candidates {
        if existing.contains(&pid) {
            continue;
        }
        let usage = by_pid.get(&pid);
        let name = usage
            .map(|item| item.command.trim())
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string())
            .unwrap_or_else(|| format!("pid {pid}"));
        snapshot.processes.push(ProcessInfo {
            pid,
            name,
            cpu: 0.0,
            mem: 0,
            gpu_util: usage.and_then(|item| item.sm),
            gpu_mem: Some(bytes),
            cmd: None,
        });
    }

    sort_by_cpu_desc(&mut snapshot.processes);
    // 截断到 max(top_n, 10) 行：先为 GPU 显存前 N 的进程保留名额，
    // 再用 CPU 靠前的进程填满，最后仍按 CPU 降序展示。
    let limit = top_n.max(10);
    if snapshot.processes.len() > limit {
        let mut kept: Vec<ProcessInfo> = Vec::with_capacity(limit);
        for process in snapshot.processes.iter() {
            if kept.len() >= limit {
                break;
            }
            if gpu_top.contains(&process.pid) {
                kept.push(process.clone());
            }
        }
        for process in snapshot.processes.iter() {
            if kept.len() >= limit {
                break;
            }
            if !gpu_top.contains(&process.pid) {
                kept.push(process.clone());
            }
        }
        sort_by_cpu_desc(&mut kept);
        snapshot.processes = kept;
    }
    snapshot.gpu_proc_supported |= !pmon.is_empty() || !apps.is_empty();
}

fn run_pmon(program: &str) -> Option<Vec<GpuProc>> {
    match hidden_command(program)
        .args(["pmon", "-c", "1"])
        .output()
    {
        Ok(output) => Some(parse_pmon(&String::from_utf8_lossy(&output.stdout))),
        Err(_) => None,
    }
}

fn run_compute_apps(program: &str) -> Option<Vec<(u32, u64)>> {
    match hidden_command(program)
        .args([
            "--query-compute-apps=pid,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .output()
    {
        Ok(output) => Some(parse_compute_apps(&String::from_utf8_lossy(&output.stdout))),
        Err(_) => None,
    }
}

/// 本机进程级 GPU 查询：依次尝试候选 nvidia-smi 路径。
/// 不支持时返回两个空列表（不报错、不打日志）；第一个能执行的路径即返回，
/// 不再对同一二进制重复尝试。
pub fn query_gpu_processes() -> (Vec<GpuProc>, Vec<(u32, u64)>) {
    for candidate in nvidia_smi_candidates() {
        let pmon = run_pmon(&candidate);
        let apps = run_compute_apps(&candidate);
        if pmon.is_none() && apps.is_none() {
            continue;
        }
        return (pmon.unwrap_or_default(), apps.unwrap_or_default());
    }
    (Vec::new(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CpuInfo, GpuInfo, HostInfo, MemInfo};

    fn snapshot_with(processes: Vec<ProcessInfo>) -> Snapshot {
        Snapshot {
            host: HostInfo::local("本机"),
            ts: 1,
            cpu: CpuInfo::default(),
            mem: MemInfo::default(),
            gpus: Vec::new(),
            processes,
            error: None,
            gpu_proc_supported: false,
        }
    }

    fn process(pid: u32, cpu: f64) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: format!("p{pid}"),
            cpu,
            mem: 0,
            gpu_util: None,
            gpu_mem: None,
            cmd: None,
        }
    }

    #[test]
    fn parses_pmon_with_jpg_ofa_header() {
        let stdout = "\
# gpu         pid   type     sm    mem    enc    dec    jpg    ofa    command
# Idx           #    C/G      %      %      %      %      %      %    name
    0       7077     G      -      -      -      -      -      -    Xorg
    0    2828290   C+G     24      2      -      -      -      -    python
    0    3224881   C+G      -      -      -      -      -      -    python
";
        let list = parse_pmon(stdout);
        assert_eq!(list.len(), 3);
        let python = list.iter().find(|p| p.pid == 2828290).unwrap();
        assert_eq!(python.gpu, 0);
        assert_eq!(python.kind, "C+G");
        assert_eq!(python.sm, Some(24.0));
        assert_eq!(python.mem_pct, Some(2.0));
        assert_eq!(python.command, "python");
        let xorg = list.iter().find(|p| p.pid == 7077).unwrap();
        assert_eq!(xorg.sm, None);
        assert_eq!(xorg.mem_pct, None);
    }

    #[test]
    fn parses_pmon_without_jpg_ofa_columns() {
        // 老驱动只有 sm/mem/enc/dec/command，指标列仍按表头定位。
        let stdout = "\
# gpu         pid   type     sm    mem    enc    dec    command
# Idx           #    C/G      %      %      %      %    name
    1       4242     C     77      5      -      -    train
    1          x     C     10      1      -      -    broken
";
        let list = parse_pmon(stdout);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].pid, 4242);
        assert_eq!(list[0].gpu, 1);
        assert_eq!(list[0].sm, Some(77.0));
        assert_eq!(list[0].mem_pct, Some(5.0));
        assert_eq!(list[0].command, "train");
        // 没有表头也不能 panic，固定列解析失败即跳过。
        assert!(parse_pmon("garbage\n    0 12 C 5 1 - - - - x\n").len() <= 1);
    }

    #[test]
    fn parses_compute_apps_csv_with_paths_and_spaces() {
        let stdout = "\
2828289, /data/wsc/conda_envs/isaaclab45/bin/python, 1754
2828290, /data/wsc/conda envs/my,weird,dir/python, 8066
broken, /tmp/x, 10
12, /tmp/y, N/A
";
        let apps = parse_compute_apps(stdout);
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0], (2828289, 1754 * MIB));
        assert_eq!(apps[1], (2828290, 8066 * MIB));
        // 无名字的两列形式（远程脚本）也能解析
        assert_eq!(
            parse_compute_apps("7, 100\n"),
            vec![(7, 100 * MIB)]
        );
    }

    #[test]
    fn merges_by_pid_and_keeps_gpu_heavy_processes_in_top_n() {
        let mut snapshot = snapshot_with(vec![
            process(1, 120.0),
            process(2, 80.0),
            process(3, 60.0),
        ]);
        snapshot.gpus = vec![GpuInfo {
            index: 0,
            name: "RTX 4090".into(),
            util: 30.0,
            mem_used: 0,
            mem_total: 24564 * MIB,
            temp: None,
        }];
        let pmon = vec![
            GpuProc {
                pid: 2,
                gpu: 0,
                kind: "C+G".into(),
                sm: Some(24.0),
                mem_pct: Some(2.0),
                command: "python".into(),
            },
            GpuProc {
                pid: 9,
                gpu: 0,
                kind: "C".into(),
                sm: Some(1.0),
                mem_pct: Some(40.0),
                command: "idle_train".into(),
            },
        ];
        let apps = vec![(2, 8066 * MIB), (9, 9000 * MIB)];

        merge_gpu_procs(&mut snapshot, &pmon, &apps, &[24564 * MIB], 3);

        assert!(snapshot.gpu_proc_supported);
        let p2 = snapshot.processes.iter().find(|p| p.pid == 2).unwrap();
        assert_eq!(p2.gpu_util, Some(24.0));
        assert_eq!(p2.gpu_mem, Some(8066 * MIB));
        // 显存大但 CPU 低的进程被并入候选集合。
        let p9 = snapshot.processes.iter().find(|p| p.pid == 9).unwrap();
        assert_eq!(p9.name, "idle_train");
        assert_eq!(p9.cpu, 0.0);
        assert_eq!(p9.gpu_mem, Some(9000 * MIB));
        // 结果按 CPU 降序：CPU 最高的仍在最前。
        assert_eq!(snapshot.processes[0].pid, 1);
        assert_eq!(snapshot.processes.len(), 4);

        // pmon 缺显存百分比时用 compute-apps 精确值；缺 compute-apps 时用 pmon 估算。
        let mut only_pmon = snapshot_with(vec![]);
        merge_gpu_procs(&mut only_pmon, &pmon, &[], &[24564 * MIB], 10);
        let estimate = only_pmon
            .processes
            .iter()
            .find(|p| p.pid == 9)
            .unwrap();
        assert_eq!(
            estimate.gpu_mem,
            Some((24564.0 * 0.40 * MIB as f64) as u64)
        );
    }

    #[test]
    fn merge_is_noop_when_driver_reports_nothing() {
        let mut snapshot = snapshot_with(vec![process(1, 5.0)]);
        merge_gpu_procs(&mut snapshot, &[], &[], &[], 10);
        assert!(!snapshot.gpu_proc_supported);
        assert_eq!(snapshot.processes.len(), 1);
        assert_eq!(snapshot.processes[0].gpu_util, None);
        assert_eq!(snapshot.processes[0].gpu_mem, None);
    }

    #[test]
    fn gpu_heavy_process_survives_top_n_truncation() {
        // CPU 前 10 已占满表格；显存很大的低 CPU 进程仍必须保留，
        // 由它挤掉 CPU 排名最后的一行。
        let cpu_rows: Vec<ProcessInfo> = (1..=10)
            .map(|pid| process(pid, 110.0 - pid as f64 * 10.0))
            .collect();
        let mut snapshot = snapshot_with(cpu_rows);
        snapshot.gpus = vec![GpuInfo {
            index: 0,
            name: "RTX 4090".into(),
            util: 5.0,
            mem_used: 0,
            mem_total: 24564 * MIB,
            temp: None,
        }];
        let pmon = vec![GpuProc {
            pid: 99,
            gpu: 0,
            kind: "C".into(),
            sm: Some(1.0),
            mem_pct: Some(40.0),
            command: "idle_train".into(),
        }];
        merge_gpu_procs(
            &mut snapshot,
            &pmon,
            &[(99, 9800 * MIB)],
            &[24564 * MIB],
            10,
        );

        assert_eq!(snapshot.processes.len(), 10);
        let kept = snapshot.processes.iter().find(|p| p.pid == 99).unwrap();
        assert_eq!(kept.gpu_mem, Some(9800 * MIB));
        assert_eq!(kept.gpu_util, Some(1.0));
        assert!(snapshot.processes.iter().all(|p| p.pid != 10));
        // 保留的行仍按 CPU 降序
        assert_eq!(snapshot.processes[0].pid, 1);
        assert!(snapshot
            .processes
            .windows(2)
            .all(|pair| pair[0].cpu >= pair[1].cpu));
    }
}
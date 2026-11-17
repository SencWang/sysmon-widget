//! 采集层：sysinfo 取 CPU/内存/进程，nvidia-smi 子进程取 GPU。
use std::collections::VecDeque;
use std::process::Command;

use sysinfo::{ProcessesToUpdate, System};

use crate::model::{
    percent, CpuInfo, GpuInfo, HistoryPayload, HostInfo, MemInfo, ProcessInfo, Snapshot,
};

/// nvidia-smi 查询参数（不拼接字符串，避免 cmd /c）。
pub const GPU_QUERY_ARGS: [&str; 2] = [
    "--query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu",
    "--format=csv,noheader,nounits",
];

const MIB: u64 = 1024 * 1024;

/// 构造隐藏控制台的 Command（Windows 下 CREATE_NO_WINDOW = 0x08000000）。
pub fn hidden_command(program: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new(program);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

/// 解析一行 nvidia-smi CSV；字段不足或数字非法时返回 None（整行跳过）。
pub fn parse_gpu_line(line: &str) -> Option<GpuInfo> {
    let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
    if parts.len() < 6 {
        return None;
    }
    let index: u32 = parts[0].parse().ok()?;
    let name = parts[1].to_string();
    if name.is_empty() {
        return None;
    }
    let util: f64 = parts[2].parse().ok()?;
    let mem_used_mib: f64 = parts[3].parse().ok()?;
    let mem_total_mib: f64 = parts[4].parse().ok()?;
    // 温度可能是 [N/A]
    let temp = parts[5].parse::<f64>().ok();
    Some(GpuInfo {
        index,
        name,
        util: util.clamp(0.0, 100.0),
        mem_used: (mem_used_mib.max(0.0) as u64) * MIB,
        mem_total: (mem_total_mib.max(0.0) as u64) * MIB,
        temp,
    })
}

/// 解析整段 stdout，坏行跳过，保持原有顺序。
pub fn parse_nvidia_smi_csv(stdout: &str) -> Vec<GpuInfo> {
    stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .filter_map(parse_gpu_line)
        .collect()
}

pub(crate) fn nvidia_smi_candidates() -> Vec<String> {
    let mut list = vec!["nvidia-smi".to_string()];
    #[cfg(windows)]
    {
        let system32 = r"C:\Windows\System32\nvidia-smi.exe";
        if std::path::Path::new(system32).exists() {
            list.push(system32.to_string());
        }
    }
    list
}

/// nvidia-smi 单次执行结果。
enum SmiOutcome {
    /// 正常输出
    Output(String),
    /// 工具存在但没有 GPU（"No devices were found"），不算错误
    NoDevice,
    /// 工具存在但执行失败
    Failed(String),
    /// 找不到可执行文件
    Missing,
}

fn run_nvidia_smi(program: &str) -> SmiOutcome {
    let output = match hidden_command(program).args(GPU_QUERY_ARGS).output() {
        Ok(output) => output,
        Err(_) => return SmiOutcome::Missing,
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        return SmiOutcome::Output(stdout);
    }
    let detail = format!("{stdout}\n{stderr}").to_lowercase();
    if detail.contains("no devices") || detail.contains("not found") {
        return SmiOutcome::NoDevice;
    }
    SmiOutcome::Failed(format!(
        "nvidia-smi 执行失败（退出码 {}）",
        output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "未知".to_string())
    ))
}

/// 用指定程序查询 GPU，返回 (GPU 列表, 错误信息)。
/// 工具缺失或没有 GPU 时返回空列表且 error 为 None；只有在工具可执行却失败/解析失败时才给出错误。
pub fn query_gpus_with(program: &str) -> (Vec<GpuInfo>, Option<String>) {
    match run_nvidia_smi(program) {
        SmiOutcome::Output(stdout) => {
            let gpus = parse_nvidia_smi_csv(&stdout);
            if gpus.is_empty() && !stdout.trim().is_empty() {
                (
                    Vec::new(),
                    Some("nvidia-smi 输出解析失败（无有效数据行）".to_string()),
                )
            } else {
                (gpus, None)
            }
        }
        SmiOutcome::NoDevice => (Vec::new(), None),
        SmiOutcome::Failed(msg) => (Vec::new(), Some(msg)),
        SmiOutcome::Missing => (Vec::new(), None),
    }
}

/// 依次尝试候选路径；nvidia-smi 缺失时 gpus 为空数组且不报错（不 panic、不弹窗）。
pub fn query_gpus() -> (Vec<GpuInfo>, Option<String>) {
    let mut last_error: Option<String> = None;
    for candidate in nvidia_smi_candidates() {
        let (gpus, error) = query_gpus_with(&candidate);
        if !gpus.is_empty() {
            return (gpus, None);
        }
        if error.is_some() {
            last_error = error;
        }
    }
    (Vec::new(), last_error)
}

/// 采集器：持有 sysinfo 实例，按 refresh 间隔更新。
pub struct Collector {
    sys: Option<System>,
    host: HostInfo,
    init_error: Option<String>,
}

impl Collector {
    pub fn new(host_label: &str) -> Self {
        let host = HostInfo::local(host_label);
        match System::new_all() {
            Ok(sys) => Self {
                sys: Some(sys),
                host,
                init_error: None,
            },
            Err(err) => {
                eprintln!("[sysmon] sysinfo 初始化失败: {err}");
                Self {
                    sys: None,
                    host,
                    init_error: Some(format!("sysinfo 初始化失败: {err}")),
                }
            }
        }
    }

    pub fn set_label(&mut self, label: &str) {
        self.host.label = label.to_string();
    }

    /// CPU 使用率依赖两次采样差值，启动时先刷新两次（约 200ms 间隔）。
    pub fn prime(&mut self) {
        if let Some(sys) = self.sys.as_mut() {
            sys.refresh_cpu_usage();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(
                std::time::Duration::from_millis(200),
            ));
            sys.refresh_cpu_usage();
        }
    }

    pub fn collect(&mut self, top_n: usize) -> Snapshot {
        let Some(sys) = self.sys.as_mut() else {
            return Snapshot {
                host: self.host.clone(),
                ts: crate::model::now_ms(),
                cpu: CpuInfo::default(),
                mem: MemInfo::default(),
                gpus: Vec::new(),
                processes: Vec::new(),
                error: self
                    .init_error
                    .clone()
                    .or_else(|| Some("采集器未初始化".to_string())),
                gpu_proc_supported: false,
            };
        };

        let mut errors: Vec<String> = Vec::new();

        sys.refresh_cpu_usage();
        sys.refresh_memory();
        sys.refresh_processes(ProcessesToUpdate::All, true);

        let per_core: Vec<f64> = sys.cpus().iter().map(|c| c.usage() as f64).collect();
        let cpu = CpuInfo {
            usage: sys.global_cpu_usage() as f64,
            cores: per_core.len(),
            per_core,
        };

        let mem = MemInfo {
            used: sys.used_memory(),
            total: sys.total_memory(),
        };

        let mut processes: Vec<ProcessInfo> = sys
            .processes()
            .iter()
            .map(|(pid, proc_)| ProcessInfo {
                pid: pid.as_u32(),
                name: proc_.name().to_string_lossy().to_string(),
                cpu: proc_.cpu_usage() as f64,
                mem: proc_.memory(),
                gpu_util: None,
                gpu_mem: None,
            })
            .collect();
        processes.sort_by(|a, b| {
            b.cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.mem.cmp(&a.mem))
        });
        if top_n > 0 && processes.len() > top_n {
            processes.truncate(top_n);
        }

        let (gpus, gpu_error) = query_gpus();
        if let Some(err) = gpu_error {
            errors.push(err);
        }

        // 进程级 GPU：pmon 失败时返回空列表，不影响上面的采集结果。
        let (pmon, apps) = crate::gpu_proc::query_gpu_processes();
        let gpu_total_bytes: Vec<u64> = gpus.iter().map(|gpu| gpu.mem_total).collect();

        let mut snapshot = Snapshot {
            host: self.host.clone(),
            ts: crate::model::now_ms(),
            cpu,
            mem,
            gpus,
            processes,
            error: if errors.is_empty() {
                None
            } else {
                Some(errors.join("；"))
            },
            gpu_proc_supported: false,
        };
        crate::gpu_proc::merge_gpu_procs(
            &mut snapshot,
            &pmon,
            &apps,
            &gpu_total_bytes,
            top_n.max(1),
        );
        snapshot
    }
}

/// 由快照序列生成历史曲线；CPU/内存/GPU/显存均换算成 0-100 百分比。
pub fn build_history(snapshots: &VecDeque<Snapshot>) -> HistoryPayload {
    let mut payload = HistoryPayload::default();
    for snap in snapshots {
        payload.ts.push(snap.ts);
        payload.cpu.push(snap.cpu.usage.clamp(0.0, 100.0));
        payload.mem.push(percent(snap.mem.used, snap.mem.total));
        let gpu = snap.gpus.first();
        payload.gpu.push(gpu.map(|g| g.util.clamp(0.0, 100.0)).unwrap_or(0.0));
        payload
            .vram
            .push(gpu.map(|g| percent(g.mem_used, g.mem_total)).unwrap_or(0.0));
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nvidia_smi_csv_rows() {
        let stdout = "0, NVIDIA GeForce RTX 3050 Laptop GPU, 37, 1536, 4096, 61\n";
        let gpus = parse_nvidia_smi_csv(stdout);
        assert_eq!(gpus.len(), 1);
        let gpu = &gpus[0];
        assert_eq!(gpu.index, 0);
        assert_eq!(gpu.name, "NVIDIA GeForce RTX 3050 Laptop GPU");
        assert!((gpu.util - 37.0).abs() < f64::EPSILON);
        assert_eq!(gpu.mem_used, 1536 * MIB);
        assert_eq!(gpu.mem_total, 4096 * MIB);
        assert_eq!(gpu.temp, Some(61.0));
    }

    #[test]
    fn skips_broken_rows_and_handles_na_temperature() {
        let stdout = "\
0, GPU A, 10, 100, 2048, 45
garbage line
1, GPU B, N/A, 0, 8192, [N/A]
2, GPU C, 55, 512, 8192, [N/A]
";
        let gpus = parse_nvidia_smi_csv(stdout);
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[0].name, "GPU A");
        assert_eq!(gpus[1].name, "GPU C");
        assert_eq!(gpus[1].temp, None);
        assert_eq!(gpus[1].mem_total, 8192 * MIB);
    }

    #[test]
    fn missing_gpu_falls_back_to_empty_list() {
        // 空输出、坏输出以及不存在的可执行文件都必须安全回退，且不把“无 GPU”当成错误
        assert!(parse_nvidia_smi_csv("").is_empty());
        assert!(parse_nvidia_smi_csv("No devices were found\n").is_empty());
        let (gpus, error) = query_gpus_with("definitely-not-a-real-nvidia-smi-binary");
        assert!(gpus.is_empty());
        assert!(error.is_none());
    }

    #[test]
    fn history_percentages_are_normalized() {
        let mut queue: VecDeque<Snapshot> = VecDeque::new();
        queue.push_back(Snapshot {
            host: HostInfo::local("本机"),
            ts: 1000,
            cpu: CpuInfo {
                usage: 42.0,
                per_core: vec![42.0],
                cores: 1,
            },
            mem: MemInfo {
                used: 8 * 1024 * MIB,
                total: 16 * 1024 * MIB,
            },
            gpus: vec![GpuInfo {
                index: 0,
                name: "GPU".into(),
                util: 30.0,
                mem_used: MIB,
                mem_total: 4 * MIB,
                temp: None,
            }],
            processes: vec![],
            error: None,
            gpu_proc_supported: false,
        });
        let hist = build_history(&queue);
        assert_eq!(hist.ts, vec![1000]);
        assert!((hist.cpu[0] - 42.0).abs() < f64::EPSILON);
        assert!((hist.mem[0] - 50.0).abs() < f64::EPSILON);
        assert!((hist.gpu[0] - 30.0).abs() < f64::EPSILON);
        assert!((hist.vram[0] - 25.0).abs() < f64::EPSILON);
    }
}
// 与 Rust 后端一致的数据模型（serde rename_all = camelCase）
export type ViewName = "tile" | "detail" | "grid";
export type Level = "normal" | "warn" | "danger";
export type SortKey = "cpu" | "mem" | "gpu" | "gpuMem";

export type HostInfo = {
  id: string;
  label: string;
  kind: "local" | "ssh";
};

export type CpuInfo = {
  /** 0-100 */
  usage: number;
  perCore: number[];
  cores: number;
};

export type MemInfo = {
  /** bytes */
  used: number;
  total: number;
};

export type GpuInfo = {
  index: number;
  name: string;
  /** 0-100 */
  util: number;
  /** bytes */
  memUsed: number;
  memTotal: number;
  temp: number | null;
};

export type ProcessInfo = {
  pid: number;
  name: string;
  /** 可能 > 100（多核） */
  cpu: number;
  /** bytes */
  mem: number;
  /** 进程级 GPU 利用率 0-100；主机/驱动不支持时为 null */
  gpuUtil?: number | null;
  /** 进程级显存（bytes）；主机/驱动不支持时为 null */
  gpuMem?: number | null;
};

export type Snapshot = {
  host: HostInfo;
  /** epoch ms */
  ts: number;
  cpu: CpuInfo;
  mem: MemInfo;
  gpus: GpuInfo[];
  processes: ProcessInfo[];
  error: string | null;
  /** 本轮是否拿到任意进程级 GPU 数据（WDDM 等为 false） */
  gpuProcSupported?: boolean;
};

export type ServerConfig = {
  id: string;
  label: string;
  alias: string;
  enabled: boolean;
};

export type AppConfig = {
  refreshMs: number;
  historyPoints: number;
  topN: number;
  opacity: number;
  alwaysOnTop: boolean;
  /** 开机自启（真实状态以注册表为准） */
  autostart: boolean;
  view: ViewName;
  /** tile 当前展示的主机 id：`local` 或 `ssh:<alias>` */
  tileHostId: string;
  localLabel: string;
  servers: ServerConfig[];
};

export type HistoryPayload = {
  ts: number[];
  cpu: number[];
  mem: number[];
  gpu: number[];
  vram: number[];
};

/** 远程主机运行态（本机也用同一结构传给 grid/detail） */
export type HostRuntime = {
  host: HostInfo;
  online: boolean;
  latencyMs: number | null;
  lastOkTs: number | null;
  /** 最近一次成功采集（离线时保留旧数据） */
  snapshot: Snapshot | null;
  error: string | null;
};

export type SshAlias = {
  alias: string;
  hostname: string;
  user: string;
};

export type TestResult = {
  ok: boolean;
  latencyMs: number | null;
  message: string;
};

/** `sync_ssh_servers` 返回值 */
export type SyncResult = {
  added: string[];
  total: number;
};

export const emptyHistory = (): HistoryPayload => ({
  ts: [],
  cpu: [],
  mem: [],
  gpu: [],
  vram: [],
});

export const COLORS = {
  cpu: "#38bdf8",
  gpu: "#a78bfa",
  mem: "#34d399",
  warn: "#fbbf24",
  danger: "#f87171",
  text: "#f0f6fc",
  dim: "#aeb9c6",
} as const;

/** <60 正常，60-85 警告，>85 危险 */
export function levelFor(usage: number): Level {
  if (usage > 85) return "danger";
  if (usage >= 60) return "warn";
  return "normal";
}

export function levelColor(usage: number): string {
  const level = levelFor(usage);
  if (level === "danger") return COLORS.danger;
  if (level === "warn") return COLORS.warn;
  return COLORS.cpu;
}

/** 主机状态点：在线绿 / 离线红（0.2.0 起离线条目统一用红点 + 原因文字） */
export function hostStatus(host: HostRuntime): { color: string; label: string } {
  if (host.online) return { color: COLORS.mem, label: "在线" };
  return { color: COLORS.danger, label: "离线" };
}
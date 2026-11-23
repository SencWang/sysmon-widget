export function clamp(n: number, min: number, max: number): number {
  if (!Number.isFinite(n)) return min;
  return Math.min(max, Math.max(min, n));
}

/** bytes -> GiB 字符串 */
export function gib(bytes: number, digits = 1): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0.0";
  return (bytes / 1024 / 1024 / 1024).toFixed(digits);
}

export function pct(value: number, digits = 0): string {
  return `${clamp(value, 0, 100).toFixed(digits)}%`;
}

export function timeOf(ts: number): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return "--:--:--";
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** 进程 CPU 可能 >100，图表条按 100 截断但数值照实显示 */
export function shortName(name: string, max = 28): string {
  if (name.length <= max) return name;
  return `${name.slice(0, max - 1)}…`;
}

/** 取路径最后一段（兼容 `/data/x.py` 与 `C:\\x\\y.py`） */
function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? "";
}

function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max)}…` : text;
}

/**
 * 从完整命令行提取「脚本/程序」短标签：跳过 argv[0]（解释器/可执行文件）和
 * 以 `-` 开头的选项，取第一个非选项参数的文件名；`python -c ...` 取代码前 24 字符。
 * 没有可用参数时返回 null，界面只显示进程名。
 */
export function cmdTag(cmd?: string | null): string | null {
  const text = cmd?.trim();
  if (!text) return null;
  const tokens = text.split(/\s+/);
  if (tokens.length < 2) return null;
  const args = tokens.slice(1);
  const codeIndex = args.indexOf("-c");
  if (codeIndex >= 0) {
    const code = args.slice(codeIndex + 1).join(" ").trim();
    return code ? clip(code, 24) : null;
  }
  for (const arg of args) {
    if (arg.startsWith("-")) continue;
    const base = baseName(arg);
    if (base) return base;
  }
  return null;
}

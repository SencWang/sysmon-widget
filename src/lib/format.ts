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
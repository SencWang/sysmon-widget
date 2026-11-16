import { useMemo } from "react";
import { clamp, gib, shortName } from "../lib/format";
import { COLORS, type ProcessInfo, type SortKey } from "../types";

type Props = {
  processes: ProcessInfo[];
  sortKey: SortKey;
  onSort: (key: SortKey) => void;
  memTotal: number;
  /** 可选说明（远程主机标注采样口径） */
  note?: string;
};

/** Top10 进程表：点击表头切换 CPU/内存排序，排序状态由上层保存 */
export function ProcessTable({ processes, sortKey, onSort, memTotal, note }: Props) {
  const rows = useMemo(() => {
    const copy = [...processes];
    copy.sort((a, b) => (sortKey === "cpu" ? b.cpu - a.cpu : b.mem - a.mem));
    return copy.slice(0, 10);
  }, [processes, sortKey]);

  const maxCpu = Math.max(100, ...rows.map((r) => r.cpu));
  const memBase = memTotal > 0 ? memTotal : Math.max(1, ...rows.map((r) => r.mem));

  return (
    <div className="proc-panel">
      {note ? <div className="proc-note dim">{note}</div> : null}
      <div className="proc-scroll">
        <table className="proc-table">
          <thead>
            <tr>
              <th className="col-pid">PID</th>
              <th className="col-name">进程</th>
              <th
                className={`col-num sortable ${sortKey === "cpu" ? "active" : ""}`}
                onClick={() => onSort("cpu")}
                title="按 CPU 排序"
              >
                CPU {sortKey === "cpu" ? "▼" : ""}
              </th>
              <th
                className={`col-num sortable ${sortKey === "mem" ? "active" : ""}`}
                onClick={() => onSort("mem")}
                title="按内存排序"
              >
                内存 {sortKey === "mem" ? "▼" : ""}
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => (
              <tr key={`${p.pid}-${p.name}`}>
                <td className="col-pid mono dim">{p.pid}</td>
                <td className="col-name" title={p.name}>
                  {shortName(p.name, 26)}
                </td>
                <td className="col-num">
                  <div className="cell">
                    <span className="mono">{p.cpu.toFixed(1)}%</span>
                    <div className="mini-track">
                      <div
                        className="mini-fill"
                        style={{
                          width: `${clamp((p.cpu / maxCpu) * 100, 1, 100)}%`,
                          background: COLORS.cpu,
                        }}
                      />
                    </div>
                  </div>
                </td>
                <td className="col-num">
                  <div className="cell">
                    <span className="mono">{gib(p.mem)} GiB</span>
                    <div className="mini-track">
                      <div
                        className="mini-fill"
                        style={{
                          width: `${clamp((p.mem / memBase) * 100, 1, 100)}%`,
                          background: COLORS.mem,
                        }}
                      />
                    </div>
                  </div>
                </td>
              </tr>
            ))}
            {rows.length === 0 ? (
              <tr>
                <td className="empty dim" colSpan={4}>
                  暂未采集到进程数据
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
    </div>
  );
}
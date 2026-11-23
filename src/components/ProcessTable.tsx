import { useMemo, useState } from "react";
import { clamp, cmdTag, gib, shortName } from "../lib/format";
import { COLORS, type ProcessInfo, type SortKey } from "../types";

type Props = {
  processes: ProcessInfo[];
  sortKey: SortKey;
  onSort: (key: SortKey) => void;
  memTotal: number;
  /** 可选说明（远程主机标注采样口径） */
  note?: string;
  /** 本轮是否拿到进程级 GPU 数据；false 时在表下给出驱动限制说明 */
  gpuProcSupported?: boolean;
  /** 最多显示行数（默认 10；由 max(topN, 10) 决定） */
  limit?: number;
};

const NO_GPU_TITLE = "该主机或驱动未提供进程级 GPU 数据";

function sortValue(process: ProcessInfo, key: SortKey): number | null {
  if (key === "cpu") return process.cpu;
  if (key === "mem") return process.mem;
  if (key === "gpu") return process.gpuUtil ?? null;
  return process.gpuMem ?? null;
}

/** Top10 进程表：点击表头在 CPU / 内存 / GPU% / GPU 显存之间切换排序 */
export function ProcessTable({
  processes,
  sortKey,
  onSort,
  memTotal,
  note,
  gpuProcSupported,
  limit = 10,
}: Props) {
  const [descending, setDescending] = useState(true);

  const rows = useMemo(() => {
    const copy = [...processes];
    copy.sort((a, b) => {
      const left = sortValue(a, sortKey);
      const right = sortValue(b, sortKey);
      // 缺少 GPU 数据的行无论升降序都排在最后。
      if (left == null && right == null) return 0;
      if (left == null) return 1;
      if (right == null) return -1;
      return descending ? right - left : left - right;
    });
    return copy.slice(0, Math.max(limit, 1));
  }, [processes, sortKey, descending, limit]);

  const handleSort = (key: SortKey) => {
    if (key === sortKey) {
      setDescending((value) => !value);
    } else {
      onSort(key);
      setDescending(true);
    }
  };

  const arrow = (key: SortKey) =>
    sortKey === key ? (descending ? " ▼" : " ▲") : "";

  const maxCpu = Math.max(100, ...rows.map((r) => r.cpu));
  const memBase = memTotal > 0 ? memTotal : Math.max(1, ...rows.map((r) => r.mem));
  const gpuMemBase = Math.max(1, ...rows.map((r) => r.gpuMem ?? 0));

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
                onClick={() => handleSort("cpu")}
                title="按 CPU 排序"
              >
                CPU{arrow("cpu")}
              </th>
              <th
                className={`col-num sortable ${sortKey === "mem" ? "active" : ""}`}
                onClick={() => handleSort("mem")}
                title="按内存排序"
              >
                内存{arrow("mem")}
              </th>
              <th
                className={`col-gpu sortable ${sortKey === "gpu" ? "active" : ""}`}
                onClick={() => handleSort("gpu")}
                title="nvidia-smi pmon SM 利用率 / compute-apps 显存"
              >
                GPU%{arrow("gpu")}
              </th>
              <th
                className={`col-gpu sortable ${sortKey === "gpuMem" ? "active" : ""}`}
                onClick={() => handleSort("gpuMem")}
                title="按进程显存（nvidia-smi compute-apps）排序"
              >
                GPU 显存{arrow("gpuMem")}
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => {
              const tag = cmdTag(p.cmd);
              return (
                <tr key={`${p.pid}-${p.name}`}>
                  <td className="col-pid mono dim">{p.pid}</td>
                  <td className="col-name" title={p.cmd?.trim() || p.name}>
                    <span className="proc-name">{shortName(p.name, 26)}</span>
                    {tag ? <span className="proc-tag"> · {tag}</span> : null}
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
                  <td className="col-gpu">
                    {p.gpuUtil == null ? (
                      <span className="dim" title={NO_GPU_TITLE}>
                        —
                      </span>
                    ) : (
                      <div className="cell">
                        <span className="mono">{Math.round(p.gpuUtil)}%</span>
                        <div className="mini-track">
                          <div
                            className="mini-fill"
                            style={{
                              width: `${clamp(p.gpuUtil, 1, 100)}%`,
                              background: COLORS.gpu,
                            }}
                          />
                        </div>
                      </div>
                    )}
                  </td>
                  <td className="col-gpu">
                    {p.gpuMem == null ? (
                      <span className="dim" title={NO_GPU_TITLE}>
                        —
                      </span>
                    ) : (
                      <div className="cell">
                        <span className="mono">{gib(p.gpuMem)} GiB</span>
                        <div className="mini-track">
                          <div
                            className="mini-fill"
                            style={{
                              width: `${clamp((p.gpuMem / gpuMemBase) * 100, 1, 100)}%`,
                              background: COLORS.gpu,
                            }}
                          />
                        </div>
                      </div>
                    )}
                  </td>
                </tr>
              );
            })}
            {rows.length === 0 ? (
              <tr>
                <td className="empty dim" colSpan={6}>
                  暂未采集到进程数据
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
      {gpuProcSupported === false ? (
        <div className="proc-gpu-note dim">
          当前主机/驱动不支持按进程统计 GPU（Windows WDDM 常见）
        </div>
      ) : null}
    </div>
  );
}
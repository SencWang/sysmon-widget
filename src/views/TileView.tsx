import { Bar } from "../components/Bar";
import { Ring } from "../components/Ring";
import { Sparkline } from "../components/Sparkline";
import { gib, pct, timeOf } from "../lib/format";
import {
  COLORS,
  hostStatus,
  levelColor,
  type HistoryPayload,
  type HostRuntime,
  type Snapshot,
} from "../types";

type Props = {
  /** 本机 + 全部启用远程主机（本机在前） */
  hosts: HostRuntime[];
  /** 当前 tile 展示的主机 */
  host: HostRuntime;
  history: HistoryPayload;
  onExpand: () => void;
  onGrid: () => void;
  onPickHost: (hostId: string) => void;
};

/** chip 上的 CPU%/GPU% 摘要：离线显示“离线”，无 GPU 显示“--” */
function chipMetrics(entry: HostRuntime): { text: string; offline: boolean } {
  if (!entry.online) return { text: "离线", offline: true };
  const snap = entry.snapshot;
  const cpu = snap ? snap.cpu.usage : 0;
  const gpu = snap?.gpus[0];
  return {
    text: `${cpu.toFixed(0)}%/${gpu ? `${gpu.util.toFixed(0)}%` : "--"}`,
    offline: false,
  };
}

/** tile：400x280 贴片，上半部分当前主机，下方多主机 chips */
export function TileView({ hosts, host, history, onExpand, onGrid, onPickHost }: Props) {
  const snapshot: Snapshot | null = host.snapshot;
  const online = host.online;
  const cpu = snapshot?.cpu.usage ?? 0;
  const gpu = snapshot?.gpus[0];
  const memUsed = snapshot?.mem.used ?? 0;
  const memTotal = snapshot?.mem.total ?? 0;
  const memPct = memTotal > 0 ? (memUsed / memTotal) * 100 : 0;
  const vramPct = gpu && gpu.memTotal > 0 ? (gpu.memUsed / gpu.memTotal) * 100 : 0;
  const status = hostStatus(host);

  const sparkCpu = history.cpu.slice(-60);
  const sparkGpu = history.gpu.slice(-60);

  return (
    <div className="tile" data-tauri-drag-region onDoubleClick={onExpand}>
      <header className="tile-head" data-tauri-drag-region>
        <div className="tile-host" data-tauri-drag-region>
          <span
            className="dot"
            style={{ background: status.color, color: status.color }}
            title={status.label}
          />
          <span className="host-name" data-tauri-drag-region title={host.host.label}>
            {host.host.label}
          </span>
          <span className="mono dim" data-tauri-drag-region>
            {snapshot ? timeOf(snapshot.ts) : "--:--:--"}
          </span>
        </div>
        <div className="tile-actions">
          <button
            className="icon-btn"
            title="多机总览（grid）"
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.stopPropagation();
              onGrid();
            }}
          >
            ▦
          </button>
          <button
            className="icon-btn"
            title="展开详情（detail）"
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => {
              e.stopPropagation();
              onExpand();
            }}
          >
            ⤢
          </button>
        </div>
      </header>

      <div className="tile-body">
        <div className="rings">
          <Ring value={cpu} color={COLORS.cpu} label="CPU" caption={pct(cpu)} />
          <Ring
            value={gpu?.util ?? 0}
            color={COLORS.gpu}
            label="GPU"
            caption={
              gpu
                ? `${gpu.temp != null ? `${gpu.temp.toFixed(0)}°C` : "温度 --"}`
                : online
                  ? "无 GPU"
                  : "离线"
            }
          />
        </div>
        <div className="tile-bars">
          <Bar
            label="内存"
            value={memPct}
            detail={memTotal > 0 ? `${gib(memUsed)}/${gib(memTotal)} GiB` : "--"}
            color={COLORS.mem}
          />
          <Bar
            label="显存"
            value={vramPct}
            detail={gpu && gpu.memTotal > 0 ? `${gib(gpu.memUsed)}/${gib(gpu.memTotal)} GiB` : "--"}
            color={COLORS.gpu}
          />
        </div>
      </div>

      <div className="tile-spark" data-tauri-drag-region>
        <Sparkline
          series={[
            { values: sparkCpu, color: COLORS.cpu },
            { values: sparkGpu, color: COLORS.gpu },
          ]}
          points={60}
          height={30}
        />
        <span className="spark-label mono dim">60s</span>
      </div>

      <div className="tile-chips" onDoubleClick={(event) => event.stopPropagation()}>
        {hosts.map((entry) => {
          const metrics = chipMetrics(entry);
          const chipStatus = hostStatus(entry);
          const active = entry.host.id === host.host.id;
          const accent = metrics.offline ? COLORS.danger : levelColor(entry.snapshot?.cpu.usage ?? 0);
          return (
            <button
              key={entry.host.id}
              className={`tile-chip ${active ? "active" : ""} ${metrics.offline ? "offline" : ""}`}
              style={active ? { borderColor: accent } : undefined}
              title={`${entry.host.label} · ${metrics.offline ? entry.error ?? "离线" : `CPU ${pct(
                entry.snapshot?.cpu.usage ?? 0,
              )} · GPU ${entry.snapshot?.gpus.length ? pct(entry.snapshot.gpus[0].util) : "--"}`}`}
              onClick={(event) => {
                event.stopPropagation();
                onPickHost(entry.host.id);
              }}
              onDoubleClick={(event) => event.stopPropagation()}
            >
              <span
                className="dot"
                style={{ background: chipStatus.color, color: chipStatus.color }}
              />
              <span className="chip-name">{entry.host.label}</span>
              <span className={`chip-metric mono ${metrics.offline ? "offline-text" : ""}`}>
                {metrics.text}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
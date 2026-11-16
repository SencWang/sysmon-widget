import { LineChart } from "../components/LineChart";
import { ProcessTable } from "../components/ProcessTable";
import { gib, pct, timeOf } from "../lib/format";
import {
  COLORS,
  hostStatus,
  levelColor,
  type AppConfig,
  type HistoryPayload,
  type HostRuntime,
  type Snapshot,
  type SortKey,
} from "../types";

type Props = {
  hosts: HostRuntime[];
  selectedId: string;
  snapshot: Snapshot | null;
  history: HistoryPayload;
  config: AppConfig | null;
  sortKey: SortKey;
  onSort: (key: SortKey) => void;
  onSelectHost: (hostId: string) => void;
  onRetry: (hostId: string) => void;
  onTile: () => void;
  onToggleTop: () => void;
};

/** detail：900x620，主机切换 + 历史曲线 + GPU 卡片 + Top10 进程表 */
export function DetailView({
  hosts,
  selectedId,
  snapshot,
  history,
  config,
  sortKey,
  onSort,
  onSelectHost,
  onRetry,
  onTile,
  onToggleTop,
}: Props) {
  const points = config?.historyPoints ?? 180;
  const selected = hosts.find((entry) => entry.host.id === selectedId) ?? hosts[0];
  const isRemote = selected.host.kind === "ssh";
  const gpus = snapshot?.gpus ?? [];
  const gpu0 = gpus[0];
  const cpu = snapshot?.cpu.usage ?? 0;
  const memUsed = snapshot?.mem.used ?? 0;
  const memTotal = snapshot?.mem.total ?? 0;
  const status = hostStatus(selected);
  const hasData = !!snapshot;

  return (
    <div className="detail">
      <header className="detail-head" data-tauri-drag-region>
        <div className="detail-title" data-tauri-drag-region>
          <span className="dot" style={{ background: status.color, color: status.color }} />
          <span className="host-name" data-tauri-drag-region>
            {selected.host.label}
          </span>
          <span className="mono dim" data-tauri-drag-region>
            {selected.online && snapshot
              ? `${timeOf(snapshot.ts)} · 刷新 ${config ? config.refreshMs / 1000 : 2}s · ${snapshot.cpu.cores} 核${
                  selected.latencyMs != null && isRemote ? ` · ${selected.latencyMs} ms` : ""
                }`
              : status.label}
          </span>
        </div>
        <div className="detail-actions">
          <button className="ghost-btn" onClick={onToggleTop}>
            置顶：{config?.alwaysOnTop ? "开" : "关"}
          </button>
          <button className="ghost-btn" onClick={onTile}>
            返回贴片
          </button>
        </div>
      </header>

      <div className="host-chips">
        {hosts.map((entry) => {
          const chipStatus = hostStatus(entry);
          return (
            <button
              key={entry.host.id}
              className={`host-chip ${entry.host.id === selectedId ? "active" : ""} ${
                entry.online ? "" : "offline"
              }`}
              onClick={() => onSelectHost(entry.host.id)}
              title={entry.error ?? entry.host.label}
            >
              <span
                className="dot"
                style={{ background: chipStatus.color, color: chipStatus.color }}
              />
              <span className="chip-name">{entry.host.label}</span>
              <span className="chip-tag mono">
                {entry.host.kind === "local"
                  ? "本机"
                  : entry.online && entry.latencyMs != null
                    ? `${entry.latencyMs}ms`
                    : "离线"}
              </span>
            </button>
          );
        })}
      </div>

      {!selected.online ? (
        <div className="offline-banner">
          <span className="dot offline" />
          <span className="offline-text" title={selected.error ?? undefined}>
            {selected.error ?? "离线（原因未知）"}
          </span>
          <button className="ghost-btn small" onClick={() => onRetry(selected.host.id)}>
            重试
          </button>
        </div>
      ) : null}

      {!hasData ? (
        <div className="detail-empty">
          <div className="detail-empty-title">
            {selected.online ? "正在采集…" : "该主机当前离线"}
          </div>
          <div className="detail-empty-text dim" title={selected.error ?? undefined}>
            {selected.error ?? "暂无数据"}
          </div>
          {isRemote ? (
            <button className="ghost-btn" onClick={() => onRetry(selected.host.id)}>
              立即重试
            </button>
          ) : null}
        </div>
      ) : (
        <div className="detail-grid">
          <LineChart
            title="CPU"
            points={points}
            right={pct(cpu)}
            series={[{ values: history.cpu, color: COLORS.cpu, label: "使用率" }]}
          />
          <LineChart
            title="内存"
            points={points}
            right={memTotal > 0 ? `${gib(memUsed)}/${gib(memTotal)} GiB` : "--"}
            series={[{ values: history.mem, color: COLORS.mem, label: "占用率" }]}
          />
          <LineChart
            title="GPU / 显存"
            points={points}
            right={gpu0 ? pct(gpu0.util) : "无 GPU"}
            series={[
              { values: history.gpu, color: COLORS.gpu, label: "GPU #0" },
              { values: history.vram, color: COLORS.mem, label: "显存 #0" },
            ]}
          />
          <section className="gpu-card">
            <header className="chart-head">
              <span className="chart-title">GPU</span>
              <span className="mono dim">
                {gpus.length > 0 ? `${gpus.length} 块 · 曲线取 #0` : "--"}
              </span>
            </header>
            {gpus.length === 0 ? (
              <div className="gpu-empty">未检测到 NVIDIA GPU</div>
            ) : (
              <div className="gpu-list">
                {gpus.map((gpu) => (
                  <div className="gpu-item" key={`${gpu.index}-${gpu.name}`}>
                    <div className="gpu-item-head">
                      <span className="gpu-badge mono">#{gpu.index}</span>
                      <span className="gpu-item-name" title={gpu.name}>
                        {gpu.name}
                      </span>
                    </div>
                    <div className="gpu-stats">
                      <div>
                        <span className="stat-label">利用率</span>
                        <span
                          className="stat-value mono"
                          style={{ color: levelColor(gpu.util) }}
                        >
                          {pct(gpu.util)}
                        </span>
                      </div>
                      <div>
                        <span className="stat-label">显存</span>
                        <span className="stat-value mono">
                          {gpu.memTotal > 0
                            ? `${gib(gpu.memUsed)}/${gib(gpu.memTotal)} GiB`
                            : "--"}
                        </span>
                      </div>
                      <div>
                        <span className="stat-label">温度</span>
                        <span className="stat-value mono">
                          {gpu.temp != null ? `${gpu.temp.toFixed(0)} °C` : "--"}
                        </span>
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>

          <ProcessTable
            processes={snapshot?.processes ?? []}
            sortKey={sortKey}
            onSort={onSort}
            memTotal={memTotal}
            note={
              isRemote
                ? "远程 Top10：CPU% 为 top -bn2 第二次采样的瞬时值（多核可超过 100%）"
                : undefined
            }
          />
        </div>
      )}
    </div>
  );
}
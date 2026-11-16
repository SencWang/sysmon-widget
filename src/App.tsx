import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "./lib/api";
import { DetailView } from "./views/DetailView";
import { GridView } from "./views/GridView";
import { TileView } from "./views/TileView";
import {
  emptyHistory,
  type AppConfig,
  type HistoryPayload,
  type HostRuntime,
  type Snapshot,
  type SortKey,
  type ViewName,
} from "./types";

function pushValue(list: number[], value: number, cap: number): number[] {
  const next = list.length >= cap ? list.slice(list.length - cap + 1) : list.slice();
  next.push(Number.isFinite(value) ? value : 0);
  return next;
}

/** 把远程快照追加进历史曲线（按 ts 去重，避免拉取历史与事件重复） */
function appendSnapshotHistory(prev: HistoryPayload, snap: Snapshot, cap: number): HistoryPayload {
  const last = prev.ts[prev.ts.length - 1];
  if (last != null && snap.ts <= last) return prev;
  const memPct = snap.mem.total > 0 ? (snap.mem.used / snap.mem.total) * 100 : 0;
  const gpu = snap.gpus[0];
  const gpuPct = gpu ? gpu.util : 0;
  const vramPct = gpu && gpu.memTotal > 0 ? (gpu.memUsed / gpu.memTotal) * 100 : 0;
  return {
    ts: pushValue(prev.ts, snap.ts, cap),
    cpu: pushValue(prev.cpu, snap.cpu.usage, cap),
    mem: pushValue(prev.mem, memPct, cap),
    gpu: pushValue(prev.gpu, gpuPct, cap),
    vram: pushValue(prev.vram, vramPct, cap),
  };
}

export default function App() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [remotes, setRemotes] = useState<HostRuntime[]>([]);
  const [config, setConfig] = useState<AppConfig | null>(null);
  /** 每台主机各自的历史曲线（本机 + 全部远程，tile/detail 共用） */
  const [histories, setHistories] = useState<Record<string, HistoryPayload>>({});
  const [selectedHostId, setSelectedHostId] = useState("local");
  const [tileHostId, setTileHostId] = useState("local");
  const [view, setView] = useState<ViewName>("tile");
  const [sortKey, setSortKey] = useState<SortKey>("cpu");
  const [ipcError, setIpcError] = useState<string | null>(null);
  const capRef = useRef(180);

  const appendToHost = useCallback((hostId: string, snap: Snapshot) => {
    setHistories((prev) => {
      const current = prev[hostId] ?? emptyHistory();
      const next = appendSnapshotHistory(current, snap, capRef.current);
      if (next === current) return prev;
      return { ...prev, [hostId]: next };
    });
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [snap, hist, remoteList] = await Promise.all([
        api.getSnapshot(),
        api.getHistory(),
        api.getRemote(),
      ]);
      if (snap) setSnapshot(snap);
      if (hist && hist.ts.length > 0) {
        setHistories((prev) => ({ ...prev, local: hist }));
      }
      setRemotes(remoteList);
      setIpcError(null);
    } catch (err) {
      setIpcError(String(err));
    }
  }, []);

  const loadRemoteHistory = useCallback(async (hostId: string) => {
    if (hostId === "local") return;
    try {
      const hist = await api.getRemoteHistory(hostId);
      if (hist && hist.ts.length > 0) {
        setHistories((prev) => ({ ...prev, [hostId]: hist }));
      }
    } catch (err) {
      setIpcError(String(err));
    }
  }, []);

  const selectHost = useCallback(
    async (hostId: string) => {
      setSelectedHostId(hostId);
      await loadRemoteHistory(hostId);
    },
    [loadRemoteHistory],
  );

  // 初始化 + 订阅后端推送
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let unRemote: (() => void) | undefined;
    let unConfig: (() => void) | undefined;

    (async () => {
      try {
        const cfg = await api.getConfig();
        if (disposed) return;
        setConfig(cfg);
        setView(cfg.view);
        setTileHostId(cfg.tileHostId || "local");
        capRef.current = cfg.historyPoints;

        const [snap, hist, remoteList] = await Promise.all([
          api.getSnapshot(),
          api.getHistory(),
          api.getRemote(),
        ]);
        if (disposed) return;
        if (snap) setSnapshot(snap);
        if (hist) setHistories((prev) => ({ ...prev, local: hist }));
        setRemotes(remoteList);
        if (cfg.tileHostId && cfg.tileHostId !== "local") {
          void loadRemoteHistory(cfg.tileHostId);
        }

        const un = await listen<Snapshot>("sysmon://snapshot", (event) => {
          if (document.hidden) return;
          setSnapshot(event.payload);
          appendToHost("local", event.payload);
        });
        if (disposed) un();
        else unlisten = un;

        const un2 = await listen<HostRuntime[]>("sysmon://remote", (event) => {
          const list = event.payload;
          setRemotes(list);
          // 所有远程主机的历史都持续累积，tile 切到任意主机都有 sparkline
          for (const entry of list) {
            if (entry.snapshot) appendToHost(entry.host.id, entry.snapshot);
          }
        });
        if (disposed) un2();
        else unRemote = un2;

        // 托盘「重新加载配置」/ 其他窗口改配置后同步到界面
        const un3 = await listen<AppConfig>("sysmon://config", (event) => {
          const next = event.payload;
          setConfig(next);
          capRef.current = next.historyPoints;
          setTileHostId(next.tileHostId || "local");
          setView(next.view);
          void api.getRemote().then(setRemotes).catch(() => undefined);
        });
        if (disposed) un3();
        else unConfig = un3;
      } catch (err) {
        setIpcError(String(err));
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
      unRemote?.();
      unConfig?.();
    };
    // 订阅只建立一次；回调里通过 setState 读取最新值
  }, [appendToHost, loadRemoteHistory]);

  useEffect(() => {
    capRef.current = config?.historyPoints ?? 180;
  }, [config]);

  // 窗口重新可见时补齐数据（隐藏期间不堆积前端内存）
  useEffect(() => {
    const onVisible = () => {
      if (!document.hidden) void refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [refresh]);

  // 主机被删除/禁用后，选中项与 tile 显示对象回落到本机
  useEffect(() => {
    if (selectedHostId === "local") return;
    const known = config?.servers.some((s) => s.enabled && s.id === selectedHostId);
    if (!known) void selectHost("local");
  }, [config, selectedHostId, selectHost]);

  useEffect(() => {
    if (tileHostId === "local") return;
    const known = config?.servers.some((s) => s.enabled && s.id === tileHostId);
    if (!known) {
      setTileHostId("local");
    }
  }, [config, tileHostId]);

  const changeView = useCallback(async (next: ViewName) => {
    setView(next);
    try {
      const cfg = await api.setView(next);
      setConfig(cfg);
      setView(cfg.view);
      setIpcError(null);
    } catch (err) {
      setIpcError(String(err));
    }
  }, []);

  const openHost = useCallback(
    async (hostId: string) => {
      await selectHost(hostId);
      await changeView("detail");
    },
    [selectHost, changeView],
  );

  /** 点击 tile 主机 chip：只切换贴片显示对象，不改窗口尺寸 */
  const pickTileHost = useCallback(
    async (hostId: string) => {
      setTileHostId(hostId);
      if (hostId !== "local") void loadRemoteHistory(hostId);
      try {
        const cfg = await api.setConfig({ tileHostId: hostId });
        setConfig(cfg);
        // 后端 sanitize 后可能与点击值不同（主机被禁用），以返回值为准
        setTileHostId(cfg.tileHostId || "local");
      } catch (err) {
        setIpcError(String(err));
      }
    },
    [loadRemoteHistory],
  );

  /** 不透明度滑块：立即生效并落盘 */
  const setOpacity = useCallback((value: number) => {
    setConfig((prev) => (prev ? { ...prev, opacity: value } : prev));
    void api
      .setConfig({ opacity: value })
      .then(setConfig)
      .catch((err) => setIpcError(String(err)));
  }, []);

  const retryHost = useCallback(async (hostId: string) => {
    if (hostId === "local") return;
    try {
      await api.refreshRemote(hostId);
    } catch (err) {
      setIpcError(String(err));
    }
  }, []);

  const toggleTop = useCallback(async () => {
    try {
      const onTop = await api.toggleAlwaysOnTop();
      setConfig((prev) => (prev ? { ...prev, alwaysOnTop: onTop } : prev));
    } catch (err) {
      setIpcError(String(err));
    }
  }, []);

  // Esc 收回 tile
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && view !== "tile") {
        void changeView("tile");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view, changeView]);

  const applyConfig = useCallback((cfg: AppConfig) => {
    setConfig(cfg);
    capRef.current = cfg.historyPoints;
    setTileHostId(cfg.tileHostId || "local");
    void api.getRemote().then(setRemotes).catch(() => undefined);
  }, []);

  const localHost = snapshot?.host ?? {
    id: "local",
    label: config?.localLabel ?? "本机",
    kind: "local" as const,
  };
  const hosts: HostRuntime[] = [
    {
      host: localHost,
      online: !!snapshot,
      latencyMs: 0,
      lastOkTs: snapshot?.ts ?? null,
      snapshot,
      error: snapshot?.error ?? null,
    },
    ...remotes,
  ];

  const selectedHost =
    hosts.find((entry) => entry.host.id === selectedHostId) ?? hosts[0];
  const selectedId = selectedHost.host.id;
  const selectedSnapshot = selectedHost.snapshot;
  const selectedHistory = histories[selectedId] ?? emptyHistory();

  const tileHost = hosts.find((entry) => entry.host.id === tileHostId) ?? hosts[0];
  const tileHistory = histories[tileHost.host.id] ?? emptyHistory();

  const errorText = snapshot?.error ?? ipcError;
  // 面板基准 alpha 0.90（默认 opacity 0.92 时正好 0.90），滑块再按比例缩放
  const opacity = config?.opacity ?? 0.92;
  const alpha = Math.min(0.98, Math.max(0.55, (0.90 * opacity) / 0.92));

  return (
    <div
      className={`app view-${view}`}
      style={{ "--panel-alpha": alpha.toFixed(3) } as React.CSSProperties}
      onContextMenu={(event) => event.preventDefault()}
    >
      {errorText ? (
        <div className="error-strip" title={errorText}>
          {errorText}
        </div>
      ) : null}

      {view === "tile" ? (
        <TileView
          hosts={hosts}
          host={tileHost}
          history={tileHistory}
          onExpand={() => void openHost(tileHost.host.id)}
          onGrid={() => void changeView("grid")}
          onPickHost={(id) => void pickTileHost(id)}
        />
      ) : null}

      {view === "detail" ? (
        <DetailView
          hosts={hosts}
          selectedId={selectedId}
          snapshot={selectedSnapshot}
          history={selectedHistory}
          config={config}
          sortKey={sortKey}
          onSort={setSortKey}
          onSelectHost={(id) => void selectHost(id)}
          onRetry={(id) => void retryHost(id)}
          onTile={() => void changeView("tile")}
          onToggleTop={() => void toggleTop()}
        />
      ) : null}

      {view === "grid" ? (
        <GridView
          hosts={hosts}
          config={config}
          onOpenHost={(id) => void openHost(id)}
          onConfigChange={applyConfig}
          onOpacity={setOpacity}
          onToggleTop={() => void toggleTop()}
        />
      ) : null}
    </div>
  );
}
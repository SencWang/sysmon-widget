import { useEffect, useRef, useState } from "react";
import { gib, pct, timeOf } from "../lib/format";
import { api } from "../lib/api";
import {
  COLORS,
  hostStatus,
  levelColor,
  type AppConfig,
  type HostRuntime,
  type SshAlias,
} from "../types";

type SettingsProps = {
  config: AppConfig | null;
  onConfig: (cfg: AppConfig) => void;
  onOpacity: (value: number) => void;
  onToggleTop: () => void;
  onClose: () => void;
};

function MiniRow({ label, value, color }: { label: string; value: number; color: string }) {
  return (
    <div className="card-row">
      <span className="card-label">{label}</span>
      <div className="card-track">
        <div
          className="card-fill"
          style={{ width: `${Math.min(100, Math.max(0, value))}%`, background: color }}
        />
      </div>
      <span className="mono card-value">{value.toFixed(0)}%</span>
    </div>
  );
}

function HostCard({ host, onOpen }: { host: HostRuntime; onOpen: () => void }) {
  const snap = host.snapshot;
  const status = hostStatus(host);
  const cpu = snap?.cpu.usage ?? 0;
  const gpu = snap?.gpus[0];
  const memTotal = snap?.mem.total ?? 0;
  const memPct = memTotal > 0 ? ((snap?.mem.used ?? 0) / memTotal) * 100 : 0;
  const vramPct = gpu && gpu.memTotal > 0 ? (gpu.memUsed / gpu.memTotal) * 100 : 0;
  const updated = host.lastOkTs ? timeOf(host.lastOkTs) : "尚未采集";
  const error = host.error;

  return (
    <button
      className={`host-card ${host.online ? "" : "offline"}`}
      onClick={onOpen}
      title={`${host.host.label} · ${error ?? status.label}`}
    >
      <div className="card-head">
        <span className="dot" style={{ background: status.color, color: status.color }} />
        <span className="card-name">{host.host.label}</span>
        <span className="card-kind mono dim">{host.host.kind === "local" ? "本机" : "SSH"}</span>
        <span className="mono card-latency dim">
          {host.host.kind === "local"
            ? status.label
            : host.online && host.latencyMs != null
              ? `${host.latencyMs} ms`
              : status.label}
        </span>
      </div>

      <MiniRow label="CPU" value={cpu} color={levelColor(cpu)} />
      <MiniRow label="GPU" value={gpu?.util ?? 0} color={COLORS.gpu} />
      <MiniRow label="内存" value={memPct} color={COLORS.mem} />
      <MiniRow label="显存" value={vramPct} color={COLORS.gpu} />

      {host.online ? (
        <div className="card-foot mono dim">
          {memTotal > 0 ? `${gib(snap?.mem.used ?? 0)}/${gib(memTotal)} GiB` : "--"}
          {gpu?.temp != null ? ` · ${gpu.temp.toFixed(0)}°C` : ""}
          {` · ${snap?.cpu.cores ?? 0} 核 · ${updated}`}
        </div>
      ) : (
        <div className="card-foot card-error mono" title={error ?? status.label}>
          {error ?? "离线（原因未知）"}
        </div>
      )}
    </button>
  );
}

/** grid：780x520 多机总览（本机 + 全部启用的远程主机） */
export function GridView({
  hosts,
  config,
  onOpenHost,
  onConfigChange,
  onOpacity,
  onToggleTop,
}: {
  hosts: HostRuntime[];
  config: AppConfig | null;
  onOpenHost: (hostId: string) => void;
  onConfigChange: (cfg: AppConfig) => void;
  onOpacity: (value: number) => void;
  onToggleTop: () => void;
}) {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const online = hosts.filter((host) => host.online).length;
  const local = hosts.find((host) => host.host.kind === "local");
  const localCpu = local?.snapshot?.cpu.usage ?? 0;

  return (
    <div className="grid-view">
      <header className="grid-head" data-tauri-drag-region>
        <div className="grid-head-left" data-tauri-drag-region>
          <span className="grid-title" data-tauri-drag-region>
            多机总览
          </span>
          <span className="mono dim" data-tauri-drag-region>
            {online}/{hosts.length} 台在线 · {pct(localCpu)} 本机 CPU
          </span>
        </div>
        <div className="grid-head-right">
          <button className="ghost-btn" onClick={() => setSettingsOpen(true)} title="远程主机设置">
            ⚙ 设置
          </button>
        </div>
      </header>

      <div className="cards">
        {hosts.map((host) => (
          <HostCard key={host.host.id} host={host} onOpen={() => onOpenHost(host.host.id)} />
        ))}
      </div>

      <footer className="grid-foot dim">
        点击卡片查看历史与 Top10 进程 · 离线原因见卡片底部 · 设置中可添加 ~/.ssh/config 主机
      </footer>

      {settingsOpen ? (
        <RemoteSettings
          config={config}
          onConfig={onConfigChange}
          onOpacity={onOpacity}
          onToggleTop={onToggleTop}
          onClose={() => setSettingsOpen(false)}
        />
      ) : null}
    </div>
  );
}

const REFRESH_OPTIONS = [2000, 3000, 5000, 10000];

/** 设置面板：读取 ~/.ssh/config 别名、增删主机、启停、改名、测试连接、刷新间隔。 */
function RemoteSettings({ config, onConfig, onOpacity, onToggleTop, onClose }: SettingsProps) {
  const [aliases, setAliases] = useState<SshAlias[]>([]);
  const [aliasError, setAliasError] = useState<string | null>(null);
  const [alias, setAlias] = useState("");
  const [label, setLabel] = useState("");
  const [toast, setToast] = useState<string | null>(null);
  const [testMsg, setTestMsg] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [autostart, setAutostart] = useState<boolean>(config?.autostart ?? false);
  const [trayText, setTrayText] = useState("");
  const toastTimer = useRef<number | undefined>(undefined);

  const notify = (message: string) => {
    setToast(message);
    if (toastTimer.current != null) window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2800);
  };

  useEffect(() => {
    let disposed = false;
    api
      .listSshAliases()
      .then((list) => {
        if (disposed) return;
        setAliases(list);
        if (list.length > 0) setAlias(list[0].alias);
      })
      .catch((err) => {
        if (!disposed) setAliasError(String(err));
      });
    // 自启的真实状态来自注册表（可能与 config.json 缓存不同）
    api
      .getAutostart()
      .then((value) => {
        if (!disposed) setAutostart(value);
      })
      .catch(() => undefined);
    // 托盘 tooltip 摘要每 5 秒刷新（与托盘线程同一实现）
    const loadTray = () => {
      api
        .getTraySummary()
        .then((text) => {
          if (!disposed) setTrayText(text);
        })
        .catch(() => undefined);
    };
    loadTray();
    const trayTimer = window.setInterval(loadTray, 5000);
    return () => {
      disposed = true;
      window.clearInterval(trayTimer);
      if (toastTimer.current != null) window.clearTimeout(toastTimer.current);
    };
  }, []);

  const doAdd = async () => {
    if (!alias.trim()) {
      notify("请选择一个 SSH 别名");
      return;
    }
    setBusy("add");
    try {
      const next = await api.addServer(alias.trim(), label.trim() || undefined);
      onConfig(next);
      setLabel("");
      notify(`已添加 ${alias.trim()}`);
    } catch (err) {
      notify(String(err));
    } finally {
      setBusy(null);
    }
  };

  /** 手动同步 ~/.ssh/config：只追加缺失主机，不删除手动配置、不改停用状态 */
  const doSync = async () => {
    setBusy("sync");
    try {
      const result = await api.syncSshServers();
      onConfig(await api.getConfig());
      if (result.added.length > 0) {
        notify(`已同步 ${result.added.length} 个新主机：${result.added.join(", ")}`);
      } else {
        notify("没有新主机（~/.ssh/config 中的别名都已添加）");
      }
    } catch (err) {
      notify(String(err));
    } finally {
      setBusy(null);
    }
  };

  const doTest = async (target: string) => {
    setBusy(`test:${target}`);
    setTestMsg((prev) => ({ ...prev, [target]: "测试中…" }));
    try {
      const result = await api.testSshAlias(target);
      setTestMsg((prev) => ({
        ...prev,
        [target]: `${result.ok ? "✓" : "✗"} ${result.message}`,
      }));
    } catch (err) {
      setTestMsg((prev) => ({ ...prev, [target]: `✗ ${String(err)}` }));
    } finally {
      setBusy(null);
    }
  };

  const doRemove = async (id: string) => {
    setBusy(`remove:${id}`);
    try {
      onConfig(await api.removeServer(id));
      notify("已删除主机");
    } catch (err) {
      notify(String(err));
    } finally {
      setBusy(null);
    }
  };

  const doToggle = async (id: string, enabled: boolean) => {
    try {
      onConfig(await api.setServerEnabled(id, enabled));
      notify(enabled ? "已启用主机" : "已禁用主机");
    } catch (err) {
      notify(String(err));
    }
  };

  const doRename = async (id: string, value: string, current: string) => {
    const next = value.trim();
    if (!next || next === current) return;
    try {
      onConfig(await api.updateServer(id, next));
      notify("已更新显示名");
    } catch (err) {
      notify(String(err));
    }
  };

  const doRefresh = async (ms: number) => {
    try {
      onConfig(await api.setConfig({ refreshMs: ms }));
      notify(`刷新间隔已设为 ${ms / 1000} 秒`);
    } catch (err) {
      notify(String(err));
    }
  };

  /** 开机自启：写/删 HKCU Run 值，后端回读注册表后返回真实状态 */
  const doAutostart = async (enabled: boolean) => {
    setBusy("autostart");
    try {
      const actual = await api.setAutostart(enabled);
      setAutostart(actual);
      notify(
        actual
          ? "已开启开机自启（HKCU\\...\\Run\\SysmonWidget）"
          : "已关闭开机自启（注册表值已删除）",
      );
    } catch (err) {
      notify(String(err));
      try {
        setAutostart(await api.getAutostart());
      } catch {
        /* 读取失败时保持原值 */
      }
    } finally {
      setBusy(null);
    }
  };

  const servers = config?.servers ?? [];
  const refreshValue = config?.refreshMs ?? 2000;
  const refreshOptions = REFRESH_OPTIONS.includes(refreshValue)
    ? REFRESH_OPTIONS
    : [...REFRESH_OPTIONS, refreshValue].sort((a, b) => a - b);

  return (
    <div className="settings-overlay" onMouseDown={onClose}>
      <section className="settings-panel" onMouseDown={(event) => event.stopPropagation()}>
        <header className="settings-head">
          <span className="settings-title">设置</span>
          <button className="icon-btn" onClick={onClose} title="关闭">
            ✕
          </button>
        </header>

        <div className="settings-body">
          <div className="settings-section">
            <div className="settings-label">外观</div>
            <div className="field-row">
              <span className="dim settings-hint">不透明度</span>
              <input
                type="range"
                className="slider"
                min={0.6}
                max={1}
                step={0.05}
                value={config?.opacity ?? 0.92}
                title="面板不透明度（0.6–1.0），拖动即时生效并保存"
                onChange={(event) => onOpacity(event.target.valueAsNumber)}
              />
              <span className="mono slider-value">{(config?.opacity ?? 0.92).toFixed(2)}</span>
              <label className="check-inline" title="窗口始终置顶">
                <input type="checkbox" checked={config?.alwaysOnTop ?? true} onChange={onToggleTop} />
                <span>置顶</span>
              </label>
            </div>
            <div className="dim settings-hint">
              面板/卡片默认 0.90 / 0.93 不透明度 + 1px 半透明白边，亮色壁纸下仍可读
            </div>
          </div>

          <div className="settings-section">
            <div className="settings-label">主机来源</div>
            <div className="field-row">
              <button className="primary-btn" onClick={() => void doSync()} disabled={busy === "sync"}>
                {busy === "sync" ? "同步中…" : "同步 SSH 别名"}
              </button>
              <span className="dim settings-hint">
                启动 / 重新加载配置时也会自动同步；只追加新主机，不删除已有项
              </span>
            </div>
          </div>

          <div className="settings-section">
            <div className="settings-label">从 ~/.ssh/config 添加主机</div>
            {aliasError ? <div className="settings-error">{aliasError}</div> : null}
            <div className="field-row">
              <select
                className="input"
                value={alias}
                onChange={(event) => setAlias(event.target.value)}
              >
                {aliases.length === 0 ? <option value="">未找到别名（检查 ~/.ssh/config）</option> : null}
                {aliases.map((item) => (
                  <option key={item.alias} value={item.alias}>
                    {item.alias}
                    {item.user || item.hostname
                      ? ` — ${item.user ? `${item.user}@` : ""}${item.hostname || "?"}`
                      : ""}
                  </option>
                ))}
              </select>
              <input
                className="input"
                placeholder="显示名（可选）"
                value={label}
                onChange={(event) => setLabel(event.target.value)}
              />
              <button className="primary-btn" onClick={() => void doAdd()} disabled={busy === "add"}>
                添加
              </button>
            </div>
          </div>

          <div className="settings-section">
            <div className="settings-label">已配置主机（{servers.length}）</div>
            <div className="server-list">
              {servers.length === 0 ? (
                <div className="settings-empty dim">尚未添加远程主机</div>
              ) : null}
              {servers.map((server) => {
                const message = testMsg[server.alias];
                return (
                  <div className={`server-row ${server.enabled ? "" : "disabled"}`} key={server.id}>
                    <label className="switch" title={server.enabled ? "点击禁用" : "点击启用"}>
                      <input
                        type="checkbox"
                        checked={server.enabled}
                        onChange={(event) => void doToggle(server.id, event.target.checked)}
                      />
                      <span />
                    </label>
                    <div className="server-main">
                      <input
                        className="input label-input"
                        defaultValue={server.label}
                        title="修改显示名（失焦保存）"
                        onBlur={(event) => void doRename(server.id, event.target.value, server.label)}
                      />
                      <div className="server-alias mono dim">
                        {server.alias}
                        {server.enabled ? "" : " · 已禁用"}
                      </div>
                      {message ? (
                        <div className={`server-test ${message.startsWith("✓") ? "ok" : "bad"}`} title={message}>
                          {message}
                        </div>
                      ) : null}
                    </div>
                    <div className="server-actions">
                      <button
                        className="ghost-btn small"
                        onClick={() => void doTest(server.alias)}
                        disabled={busy === `test:${server.alias}`}
                      >
                        {busy === `test:${server.alias}` ? "测试中" : "测试连接"}
                      </button>
                      <button
                        className="ghost-btn small danger"
                        onClick={() => void doRemove(server.id)}
                        disabled={busy === `remove:${server.id}`}
                      >
                        删除
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>

          <div className="settings-section">
            <div className="settings-label">刷新间隔</div>
            <div className="field-row">
              <select
                className="input"
                value={refreshValue}
                onChange={(event) => void doRefresh(Number(event.target.value))}
              >
                {refreshOptions.map((ms) => (
                  <option key={ms} value={ms}>
                    {ms / 1000} 秒
                  </option>
                ))}
              </select>
              <span className="dim settings-hint">
                远程主机最短 3 秒采集一次；连接失败按 5s → 10s → 30s 退避
              </span>
            </div>
          </div>

          <div className="settings-section">
            <div className="settings-label">系统</div>
            <div className="field-row">
              <label className="check-inline" title="写入 HKCU\Software\Microsoft\Windows\CurrentVersion\Run">
                <input
                  type="checkbox"
                  checked={autostart}
                  disabled={busy === "autostart"}
                  onChange={(event) => void doAutostart(event.target.checked)}
                />
                <span>开机自启（当前用户，无需管理员权限）</span>
              </label>
              {busy === "autostart" ? <span className="dim settings-hint">写入中…</span> : null}
            </div>
            <div className="dim settings-hint">
              托盘提示（每 5 秒刷新）：<span className="mono">{trayText || "--"}</span>
            </div>
            <div className="dim settings-hint">
              单实例：重复启动 exe 会唤起已存在的窗口并立即退出，系统中始终只有一套 SSH 轮询。
            </div>
          </div>
        </div>

        {toast ? <div className="toast">{toast}</div> : null}
      </section>
    </div>
  );
}
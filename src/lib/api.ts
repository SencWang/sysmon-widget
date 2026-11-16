// 前端只通过这些 invoke 与后端交互（不直接调用 window API）
import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  HistoryPayload,
  HostRuntime,
  Snapshot,
  SshAlias,
  SyncResult,
  TestResult,
  ViewName,
} from "../types";

export const api = {
  getSnapshot: () => invoke<Snapshot | null>("get_snapshot"),
  getHistory: () => invoke<HistoryPayload>("get_history"),
  getConfig: () => invoke<AppConfig>("get_config"),
  setConfig: (patch: Partial<AppConfig>) => invoke<AppConfig>("set_config", { patch }),
  setView: (view: ViewName) => invoke<AppConfig>("set_view", { view }),
  toggleAlwaysOnTop: () => invoke<boolean>("toggle_always_on_top"),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  getTraySummary: () => invoke<string>("get_tray_summary"),
  reloadConfig: () => invoke<AppConfig>("reload_config"),
  openConfigFile: () => invoke<void>("open_config_file"),
  hideWindow: () => invoke<void>("hide_window"),
  quitApp: () => invoke<void>("quit_app"),

  // ---------- 远程主机 ----------
  getRemote: () => invoke<HostRuntime[]>("get_remote"),
  getRemoteHistory: (hostId: string) => invoke<HistoryPayload>("get_remote_history", { hostId }),
  listSshAliases: () => invoke<SshAlias[]>("list_ssh_aliases"),
  testSshAlias: (alias: string) => invoke<TestResult>("test_ssh_alias", { alias }),
  addServer: (alias: string, label?: string) =>
    invoke<AppConfig>("add_server", { alias, label: label ?? null }),
  removeServer: (id: string) => invoke<AppConfig>("remove_server", { id }),
  setServerEnabled: (id: string, enabled: boolean) =>
    invoke<AppConfig>("set_server_enabled", { id, enabled }),
  updateServer: (id: string, label: string) =>
    invoke<AppConfig>("update_server", { id, label }),
  refreshRemote: (hostId: string) => invoke<void>("refresh_remote", { hostId }),
  /** 把 ~/.ssh/config 中缺失的主机追加到 servers（不删除、不改已有项） */
  syncSshServers: () => invoke<SyncResult>("sync_ssh_servers"),
};
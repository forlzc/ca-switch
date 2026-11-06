import { invoke } from "@tauri-apps/api/core";
import type { Settings, WebDavSyncSettings, S3SyncSettings, RemoteSnapshotInfo } from "@/types";

export interface ConfigTransferResult {
  success: boolean;
  message: string;
  filePath?: string;
  backupId?: string;
}

export interface WebDavTestResult {
  success: boolean;
  message?: string;
}

export interface CodexUnifyHistoryRestoreResult {
  restoredJsonlFiles: number;
  restoredStateRows: number;
  /** 还原被跳过的原因（如当前目录没有账本）；存在时不应报成功 */
  skippedReason?: string;
}

export interface WebDavSyncResult {
  status: string;
}

export const settingsApi = {
  async get(): Promise<Settings> {
    return await invoke("get_settings");
  },

  async save(settings: Settings): Promise<boolean> {
    return await invoke("save_settings", { settings });
  },

  async restart(): Promise<boolean> {
    return await invoke("restart_app");
  },

  async installUpdateAndRestart(): Promise<boolean> {
    return await invoke("install_update_and_restart");
  },

  async checkUpdates(): Promise<void> {
    await invoke("check_for_updates");
  },

  async openPortableUpdateDownload(version: string): Promise<void> {
    await invoke("open_portable_update_download", { version });
  },

  async isPortable(): Promise<boolean> {
    return await invoke("is_portable_mode");
  },

  async getClaudeCodeConfigPath(): Promise<string> {
    return await invoke("get_claude_code_config_path");
  },

  async pickDirectory(defaultPath?: string): Promise<string | null> {
    return await invoke("pick_directory", { defaultPath });
  },

  async selectConfigDirectory(defaultPath?: string): Promise<string | null> {
    return await invoke("pick_directory", { defaultPath });
  },

  async getAppConfigPath(): Promise<string> {
    return await invoke("get_app_config_path");
  },

  async openAppConfigFolder(): Promise<void> {
    await invoke("open_app_config_folder");
  },

  async revealAppLogFile(): Promise<void> {
    await invoke("reveal_app_log_file");
  },

  async getAppConfigDirOverride(): Promise<string | null> {
    return await invoke("get_app_config_dir_override");
  },

  async setAppConfigDirOverride(path: string | null): Promise<boolean> {
    return await invoke("set_app_config_dir_override", { path });
  },

  async saveFileDialog(defaultName: string): Promise<string | null> {
    return await invoke("save_file_dialog", { defaultName });
  },

  async openFileDialog(): Promise<string | null> {
    return await invoke("open_file_dialog");
  },

  async exportConfigToFile(filePath: string): Promise<ConfigTransferResult> {
    return await invoke("export_config_to_file", { filePath });
  },

  async importConfigFromFile(filePath: string): Promise<ConfigTransferResult> {
    return await invoke("import_config_from_file", { filePath });
  },

  // ─── WebDAV sync ──────────────────────────────────────────

  async webdavTestConnection(
    settings: WebDavSyncSettings,
    preserveEmptyPassword = true,
  ): Promise<WebDavTestResult> {
    return await invoke("webdav_test_connection", {
      settings,
      preserveEmptyPassword,
    });
  },

  async webdavSyncUpload(): Promise<WebDavSyncResult> {
    return await invoke("webdav_sync_upload");
  },

  async webdavSyncDownload(): Promise<WebDavSyncResult> {
    return await invoke("webdav_sync_download");
  },

  async webdavSyncSaveSettings(
    settings: WebDavSyncSettings,
    passwordTouched = false,
  ): Promise<{ success: boolean }> {
    return await invoke("webdav_sync_save_settings", {
      settings,
      passwordTouched,
    });
  },

  async webdavSyncClearSettings(): Promise<{ success: boolean }> {
    return await invoke("webdav_sync_clear_settings");
  },

  async webdavSyncFetchRemoteInfo(): Promise<
    RemoteSnapshotInfo | { empty: true }
  > {
    return await invoke("webdav_sync_fetch_remote_info");
  },

  // ===== S3 Sync API =====

  async s3TestConnection(
    settings: S3SyncSettings,
    preserveEmptyPassword = true,
  ): Promise<WebDavTestResult> {
    return await invoke("s3_test_connection", {
      settings,
      preserveEmptyPassword,
    });
  },

  async s3SyncUpload(): Promise<WebDavSyncResult> {
    return await invoke("s3_sync_upload");
  },

  async s3SyncDownload(): Promise<WebDavSyncResult> {
    return await invoke("s3_sync_download");
  },

  async s3SyncSaveSettings(
    settings: S3SyncSettings,
    passwordTouched: boolean,
  ): Promise<{ success: boolean }> {
    return await invoke("s3_sync_save_settings", {
      settings,
      passwordTouched,
    });
  },

  async s3SyncFetchRemoteInfo(): Promise<RemoteSnapshotInfo | { empty: true }> {
    return await invoke("s3_sync_fetch_remote_info");
  },

  async openExternal(url: string): Promise<void> {
    try {
      const u = new URL(url);
      const scheme = u.protocol.replace(":", "").toLowerCase();
      if (scheme !== "http" && scheme !== "https") {
        throw new Error("Unsupported URL scheme");
      }
    } catch {
      throw new Error("Invalid URL");
    }
    await invoke("open_external", { url });
  },

  async setAutoLaunch(enabled: boolean): Promise<boolean> {
    return await invoke("set_auto_launch", { enabled });
  },

  async getAutoLaunchStatus(): Promise<boolean> {
    return await invoke("get_auto_launch_status");
  },

  async getRectifierConfig(): Promise<RectifierConfig> {
    return await invoke("get_rectifier_config");
  },

  async setRectifierConfig(config: RectifierConfig): Promise<boolean> {
    return await invoke("set_rectifier_config", { config });
  },

  async getOptimizerConfig(): Promise<OptimizerConfig> {
    return await invoke("get_optimizer_config");
  },

  async setOptimizerConfig(config: OptimizerConfig): Promise<boolean> {
    return await invoke("set_optimizer_config", { config });
  },

  async getLogConfig(): Promise<LogConfig> {
    return await invoke("get_log_config");
  },

  async setLogConfig(config: LogConfig): Promise<boolean> {
    return await invoke("set_log_config", { config });
  },
};

export interface ToolUninstallPlan {
  tool: string;
  path: string;
  real_path: string;
  command: string;
  native: boolean;
  wsl_distro: string | null;
}

/** 单处工具安装的诊断信息（多处安装冲突检测）。字段对应后端 ToolInstallation。 */
export interface ToolInstallation {
  path: string;
  version: string | null;
  runnable: boolean;
  error: string | null;
  source: string;
  is_path_default: boolean;
}

/** 卸载对话框里的一处安装：能自动卸载就带 plan，否则给出原因。对应后端 ToolUninstallCandidate。 */
/** 新版本要求的 Node（`engines.node` 原文）与这处安装用的 Node 版本。 */
export interface NodeRequirementGap {
  required: string;
  current: string;
}

export interface ToolUninstallCandidate extends ToolInstallation {
  plan: ToolUninstallPlan | null;
  unavailable_reason: string | null;
  /** 工具自己的更新程序会把这份装回来（Grok 的 npm 模式），单独删掉只是暂时的。 */
  reinstalled_by_updater: boolean;
}

/** 一次"探测工具安装分布"的结果。字段对应后端 ToolInstallationReport。 */
export interface ToolInstallationReport {
  tool: string;
  installs: ToolInstallation[];
  is_conflict: boolean;
  needs_confirmation: boolean;
  command: string;
  anchored: boolean;
  /** 默认那处是认不出安装渠道的原生可执行文件：不执行升级，command 为空。 */
  unmanaged: boolean;
}

export interface RectifierConfig {
  enabled: boolean;
  requestThinkingSignature: boolean;
  requestThinkingBudget: boolean;
  requestMediaFallback: boolean;
  requestMediaHeuristic: boolean;
}

export interface OptimizerConfig {
  enabled: boolean;
  thinkingOptimizer: boolean;
  cacheInjection: boolean;
}

export interface LogConfig {
  enabled: boolean;
  level: "error" | "warn" | "info" | "debug" | "trace";
}

export interface BackupEntry {
  filename: string;
  sizeBytes: number;
  createdAt: string;
}

export interface BackupLocation {
  id: string;
  path: string;
  sizeBytes: number;
  itemCount: number;
  lastModified?: string | null;
  deletable: boolean;
}

export const backupsApi = {
  async createDbBackup(): Promise<string> {
    return await invoke("create_db_backup");
  },

  async listDbBackups(): Promise<BackupEntry[]> {
    return await invoke("list_db_backups");
  },

  async restoreDbBackup(filename: string): Promise<string> {
    return await invoke("restore_db_backup", { filename });
  },

  async renameDbBackup(oldFilename: string, newName: string): Promise<string> {
    return await invoke("rename_db_backup", { oldFilename, newName });
  },

  async deleteDbBackup(filename: string): Promise<void> {
    await invoke("delete_db_backup", { filename });
  },

  async listBackupLocations(): Promise<BackupLocation[]> {
    return await invoke("list_backup_locations");
  },

  /** 删除一类备份，返回释放的字节数 */
  async deleteBackupLocation(id: string): Promise<number> {
    return await invoke("delete_backup_location", { id });
  },

  async revealBackupLocation(id: string): Promise<boolean> {
    return await invoke("reveal_backup_location", { id });
  },
};

import { useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { providersApi, settingsApi } from "@/lib/api";
import { useSettingsQuery, useSaveSettingsMutation } from "@/lib/query";
import type { Settings } from "@/types";
import { useSettingsForm, type SettingsFormState } from "./useSettingsForm";
import {
  useDirectorySettings,
  type ResolvedDirectories,
} from "./useDirectorySettings";
import { useSettingsMetadata } from "./useSettingsMetadata";

interface SaveResult {
  requiresRestart: boolean;
}

export interface UseSettingsResult {
  settings: SettingsFormState | null;
  isLoading: boolean;
  isSaving: boolean;
  isPortable: boolean;
  appConfigDir?: string;
  /** 已保存的 CA Switch 数据目录（没覆盖时为空），用来判断有没有改动没保存 */
  initialAppConfigDir?: string;
  resolvedDirs: ResolvedDirectories;
  requiresRestart: boolean;
  updateSettings: (updates: Partial<SettingsFormState>) => void;
  updateAppConfigDir: (value?: string) => void;
  browseAppConfigDir: () => Promise<void>;
  resetAppConfigDir: () => Promise<void>;
  saveSettings: (
    overrides?: Partial<SettingsFormState>,
    options?: { silent?: boolean },
  ) => Promise<SaveResult | null>;
  autoSaveSettings: (
    updates: Partial<SettingsFormState>,
  ) => Promise<SaveResult | null>;
  resetSettings: () => void;
  acknowledgeRestart: () => void;
}

export type { SettingsFormState, ResolvedDirectories };

const sanitizeDir = (value?: string | null): string | undefined => {
  if (!value) return undefined;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
};

/**
 * useSettings - 组合层
 */
export function useSettings(): UseSettingsResult {
  const { t } = useTranslation();
  const { data } = useSettingsQuery();
  const saveMutation = useSaveSettingsMutation();

  const {
    settings,
    isLoading: isFormLoading,
    initialLanguage,
    updateSettings,
    resetSettings: resetForm,
    syncLanguage,
  } = useSettingsForm();

  const {
    appConfigDir,
    resolvedDirs,
    isLoading: isDirectoryLoading,
    initialAppConfigDir,
    commitAppConfigDir,
    updateAppConfigDir,
    browseAppConfigDir,
    resetAppConfigDir,
    resetAllDirectories,
  } = useDirectorySettings({
    settings,
    onUpdateSettings: updateSettings,
  });

  const {
    isPortable,
    requiresRestart,
    isLoading: isMetadataLoading,
    acknowledgeRestart,
    setRequiresRestart,
  } = useSettingsMetadata();

  const resetSettings = useCallback(() => {
    resetForm(data ?? null);
    syncLanguage(initialLanguage);
    resetAllDirectories();
    setRequiresRestart(false);
  }, [
    data,
    initialLanguage,
    resetForm,
    syncLanguage,
    resetAllDirectories,
    setRequiresRestart,
  ]);

  const persistLanguage = (language?: string) => {
    try {
      if (typeof window !== "undefined" && language) {
        window.localStorage.setItem("language", language);
      }
    } catch (error) {
      console.warn("[useSettings] Failed to persist language preference", error);
    }
  };

  const refreshTray = async () => {
    try {
      await providersApi.updateTrayMenu();
    } catch (error) {
      console.warn("[useSettings] Failed to refresh tray menu", error);
    }
  };

  const syncAutoLaunch = async (
    launchOnStartup: boolean | undefined,
  ) => {
    if (
      launchOnStartup === undefined ||
      launchOnStartup === data?.launchOnStartup
    ) {
      return;
    }
    try {
      await settingsApi.setAutoLaunch(launchOnStartup);
    } catch (error) {
      console.error("Failed to update auto-launch:", error);
      toast.error(
        t("settings.autoLaunchFailed", {
          defaultValue: "设置开机自启失败",
        }),
      );
    }
  };

  // 即时保存设置（General 标签页）
  const autoSaveSettings = useCallback(
    async (updates: Partial<SettingsFormState>): Promise<SaveResult | null> => {
      const mergedSettings = settings ? { ...settings, ...updates } : null;
      if (!mergedSettings) return null;

      try {
        const {
          webdavSync: _ignoredWebdavSync,
          s3Sync: _ignoredS3Sync,
          ...restSettings
        } = mergedSettings;

        const payload: Settings = {
          ...restSettings,
          language: mergedSettings.language,
        };

        await saveMutation.mutateAsync(payload);
        await syncAutoLaunch(payload.launchOnStartup);
        persistLanguage(updates.language);
        await refreshTray();

        return { requiresRestart: false };
      } catch (error) {
        console.error("[useSettings] Failed to auto-save settings", error);
        toast.error(
          t("notifications.settingsSaveFailed", {
            defaultValue: "保存设置失败: {{error}}",
            error: (error as Error)?.message ?? String(error),
          }),
        );
        throw error;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [data, saveMutation, settings, t],
  );

  // 完整保存设置（手动保存）
  const saveSettings = useCallback(
    async (
      overrides?: Partial<SettingsFormState>,
      options?: { silent?: boolean },
    ): Promise<SaveResult | null> => {
      const mergedSettings = settings ? { ...settings, ...overrides } : null;
      if (!mergedSettings) return null;
      try {
        const sanitizedAppDir = sanitizeDir(appConfigDir);
        const previousAppDir = initialAppConfigDir;
        const {
          webdavSync: _ignoredWebdavSync,
          s3Sync: _ignoredS3Sync,
          ...restSettings
        } = mergedSettings;

        const payload: Settings = {
          ...restSettings,
          language: mergedSettings.language,
        };

        await saveMutation.mutateAsync(payload);
        await settingsApi.setAppConfigDirOverride(sanitizedAppDir ?? null);
        commitAppConfigDir(sanitizedAppDir);
        await syncAutoLaunch(payload.launchOnStartup);
        persistLanguage(payload.language);
        await refreshTray();

        const appDirChanged = sanitizedAppDir !== (previousAppDir ?? undefined);
        setRequiresRestart(appDirChanged);

        if (!options?.silent) {
          toast.success(
            t("notifications.settingsSaved", {
              defaultValue: "设置已保存",
            }),
            { closeButton: true },
          );
        }

        return { requiresRestart: appDirChanged };
      } catch (error) {
        console.error("[useSettings] Failed to save settings", error);
        toast.error(
          t("notifications.settingsSaveFailed", {
            defaultValue: "保存设置失败: {{error}}",
            error: (error as Error)?.message ?? String(error),
          }),
        );
        throw error;
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [
      appConfigDir,
      commitAppConfigDir,
      data,
      initialAppConfigDir,
      saveMutation,
      settings,
      setRequiresRestart,
      t,
    ],
  );

  const isLoading = useMemo(
    () => isFormLoading || isDirectoryLoading || isMetadataLoading,
    [isFormLoading, isDirectoryLoading, isMetadataLoading],
  );

  return {
    settings,
    isLoading,
    isSaving: saveMutation.isPending,
    isPortable,
    appConfigDir,
    initialAppConfigDir,
    resolvedDirs,
    requiresRestart,
    updateSettings,
    updateAppConfigDir,
    browseAppConfigDir,
    resetAppConfigDir,
    saveSettings,
    autoSaveSettings,
    resetSettings,
    acknowledgeRestart,
  };
}

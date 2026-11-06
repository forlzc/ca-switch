import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "@/lib/toast";
import { homeDir, join } from "@tauri-apps/api/path";
import { settingsApi } from "@/lib/api";
import type { SettingsFormState } from "./useSettingsForm";

type DirectoryKey = "appConfig";

export interface ResolvedDirectories {
  appConfig: string;
}

const sanitizeDir = (value?: string | null): string | undefined => {
  if (!value) return undefined;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : undefined;
};

const computeDefaultAppConfigDir = async (): Promise<string | undefined> => {
  try {
    const home = await homeDir();
    return await join(home, ".cc-switch");
  } catch (error) {
    console.error(
      "[useDirectorySettings] Failed to resolve default app config dir",
      error,
    );
    return undefined;
  }
};

export interface UseDirectorySettingsProps {
  settings: SettingsFormState | null;
  onUpdateSettings: (updates: Partial<SettingsFormState>) => void;
}

export interface UseDirectorySettingsResult {
  appConfigDir?: string;
  resolvedDirs: ResolvedDirectories;
  isLoading: boolean;
  initialAppConfigDir?: string;
  /** 保存成功后把已保存的数据目录换成刚存的值 */
  commitAppConfigDir: (value: string | undefined) => void;
  updateAppConfigDir: (value?: string) => void;
  browseAppConfigDir: () => Promise<void>;
  resetAppConfigDir: () => Promise<void>;
  resetAllDirectories: () => void;
}

/**
 * useDirectorySettings - 目录管理
 * 精简后只保留全局数据目录（应用配置目录）。
 */
export function useDirectorySettings(
  _props: UseDirectorySettingsProps,
): UseDirectorySettingsResult {
  const { t } = useTranslation();

  const [appConfigDir, setAppConfigDir] = useState<string | undefined>(
    undefined,
  );
  const [resolvedDirs, setResolvedDirs] = useState<ResolvedDirectories>({
    appConfig: "",
  });
  const [isLoading, setIsLoading] = useState(true);

  const defaultsRef = useRef<ResolvedDirectories>({ appConfig: "" });
  const initialAppConfigDirRef = useRef<string | undefined>(undefined);
  const [initialAppConfigDir, setInitialAppConfigDir] = useState<
    string | undefined
  >(undefined);
  const commitAppConfigDir = useCallback((value: string | undefined) => {
    initialAppConfigDirRef.current = value;
    setInitialAppConfigDir(value);
  }, []);

  useEffect(() => {
    let active = true;
    setIsLoading(true);

    const load = async () => {
      try {
        const [overrideRaw, defaultAppConfig] = await Promise.all([
          settingsApi.getAppConfigDirOverride(),
          computeDefaultAppConfigDir(),
        ]);

        if (!active) return;

        const normalizedOverride = sanitizeDir(overrideRaw ?? undefined);
        defaultsRef.current = { appConfig: defaultAppConfig ?? "" };
        setAppConfigDir(normalizedOverride);
        commitAppConfigDir(normalizedOverride);
        setResolvedDirs({
          appConfig: normalizedOverride ?? defaultsRef.current.appConfig,
        });
      } catch (error) {
        console.error(
          "[useDirectorySettings] Failed to load directory info",
          error,
        );
      } finally {
        if (active) {
          setIsLoading(false);
        }
      }
    };

    void load();
    return () => {
      active = false;
    };
  }, [commitAppConfigDir]);

  const updateDirectoryState = useCallback(
    (key: DirectoryKey, value?: string) => {
      const sanitized = sanitizeDir(value);
      if (key === "appConfig") {
        setAppConfigDir(sanitized);
      }

      setResolvedDirs((prev) => {
        const next = sanitized ?? defaultsRef.current[key];
        if (prev[key] === next) return prev;
        return { ...prev, [key]: next };
      });
    },
    [],
  );

  const updateAppConfigDir = useCallback(
    (value?: string) => {
      updateDirectoryState("appConfig", value);
    },
    [updateDirectoryState],
  );

  const browseAppConfigDir = useCallback(async () => {
    const currentValue = appConfigDir ?? resolvedDirs.appConfig;
    try {
      const picked = await settingsApi.selectConfigDirectory(currentValue);
      const sanitized = sanitizeDir(picked ?? undefined);
      if (!sanitized) return;
      updateDirectoryState("appConfig", sanitized);
    } catch (error) {
      console.error(
        "[useDirectorySettings] Failed to pick app config directory",
        error,
      );
      toast.error(
        t("settings.selectFileFailed", {
          defaultValue: "选择目录失败",
        }),
      );
    }
  }, [appConfigDir, resolvedDirs.appConfig, t, updateDirectoryState]);

  const resetAppConfigDir = useCallback(async () => {
    if (!defaultsRef.current.appConfig) {
      const fallback = await computeDefaultAppConfigDir();
      if (fallback) {
        defaultsRef.current = { appConfig: fallback };
      }
    }
    updateDirectoryState("appConfig", undefined);
  }, [updateDirectoryState]);

  const resetAllDirectories = useCallback(() => {
    setAppConfigDir(initialAppConfigDirRef.current);
    setResolvedDirs({
      appConfig: initialAppConfigDirRef.current ?? defaultsRef.current.appConfig,
    });
  }, []);

  return {
    appConfigDir,
    resolvedDirs,
    isLoading,
    initialAppConfigDir,
    commitAppConfigDir,
    updateAppConfigDir,
    browseAppConfigDir,
    resetAppConfigDir,
    resetAllDirectories,
  };
}

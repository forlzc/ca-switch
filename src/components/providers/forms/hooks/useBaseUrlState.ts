import { useState, useCallback, useRef, useEffect } from "react";
import type { ProviderCategory } from "@/types";

interface UseBaseUrlStateProps {
  category: ProviderCategory | undefined;
  settingsConfig: string;
  onSettingsConfigChange: (config: string) => void;
  /**
   * 现读当前配置。`settingsConfig` 是调用方在渲染期取的快照，可能早于别处
   * （如 JSON 编辑器）的写入。改 Base URL 时若基于旧快照重建整份 JSON，就会
   * 把那些改动覆盖回去。传入本 getter 即可在事件发生时现读。
   */
  getSettingsConfig?: () => string;
}

/**
 * 管理 Anthropic 协议的 Base URL 状态（JSON 配置里的 env.ANTHROPIC_BASE_URL）。
 */
export function useBaseUrlState({
  category,
  settingsConfig,
  onSettingsConfigChange,
  getSettingsConfig,
}: UseBaseUrlStateProps) {
  const [baseUrl, setBaseUrl] = useState("");
  const isUpdatingRef = useRef(false);

  // 事件处理里一律走这里取配置：有 getter 用 getter（当前值），否则退回快照。
  const readSettingsConfig = useCallback(
    () => getSettingsConfig?.() ?? settingsConfig,
    [getSettingsConfig, settingsConfig],
  );

  // 从配置同步到 state；official 类别不显示 Base URL 输入框
  useEffect(() => {
    if (category === "official") return;
    if (isUpdatingRef.current) return;

    try {
      const config = JSON.parse(settingsConfig || "{}");
      const envUrl: unknown = config?.env?.ANTHROPIC_BASE_URL;
      const nextUrl = typeof envUrl === "string" ? envUrl.trim() : "";
      if (nextUrl !== baseUrl) {
        setBaseUrl(nextUrl);
      }
    } catch {
      // ignore
    }
  }, [category, settingsConfig, baseUrl]);

  const handleClaudeBaseUrlChange = useCallback(
    (url: string) => {
      const sanitized = url.trim();
      setBaseUrl(sanitized);
      isUpdatingRef.current = true;

      try {
        const config = JSON.parse(readSettingsConfig() || "{}");
        if (!config.env) {
          config.env = {};
        }
        config.env.ANTHROPIC_BASE_URL = sanitized;
        onSettingsConfigChange(JSON.stringify(config, null, 2));
      } catch {
        // ignore
      } finally {
        setTimeout(() => {
          isUpdatingRef.current = false;
        }, 0);
      }
    },
    [readSettingsConfig, onSettingsConfigChange],
  );

  return {
    baseUrl,
    setBaseUrl,
    handleClaudeBaseUrlChange,
  };
}

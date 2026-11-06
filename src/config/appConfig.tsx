import type { ReactNode } from "react";
import type { AppId } from "@/lib/api/types";
import type { VisibleApps } from "@/types";
import { ClaudeIcon } from "@/components/BrandIcons";

export interface AppConfig {
  label: string;
  icon: ReactNode;
  activeClass: string;
  badgeClass: string;
}

/** 精简后只保留本地代理一个入口。 */
export const APP_IDS: AppId[] = ["claude"];

export const DEFAULT_VISIBLE_APPS: VisibleApps = {
  claude: true,
};

export function orderAppIds(_saved?: readonly string[]): AppId[] {
  return [...APP_IDS];
}

export type ProxyAppId = "claude";

/** Apps with a complete local gateway + failover data plane. */
export const PROXY_APP_IDS: ProxyAppId[] = ["claude"];

export function isProxyAppId(appId: string): appId is ProxyAppId {
  return appId === "claude";
}

/** 有路由模式的应用（精简后只剩 Claude）。 */
export type ModeAppId = ProxyAppId;

export const MODE_APP_IDS: ModeAppId[] = [...PROXY_APP_IDS];

export function isModeAppId(appId: string): appId is ModeAppId {
  return appId === "claude";
}

export const APP_ICON_MAP: Record<AppId, AppConfig> = {
  claude: {
    label: "API",
    icon: <ClaudeIcon size={14} />,
    activeClass:
      "bg-orange-500/10 ring-1 ring-orange-500/20 hover:bg-orange-500/20 text-orange-600 dark:text-orange-400",
    badgeClass:
      "bg-orange-500/10 text-orange-700 dark:text-orange-300 hover:bg-orange-500/20 border-0 gap-1.5",
  },
};

export function getAppLabel(appId: string): string {
  return APP_ICON_MAP[appId as AppId]?.label ?? appId;
}

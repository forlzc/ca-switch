import type { AppId } from "@/lib/api";

/**
 * 主窗口的导航模型（精简版）：只剩一个 Claude 入口（API 管理）、用量统计和设置。
 */

export type AppPage = "providers";

export type GlobalPage = "usage";

export type View = AppPage | GlobalPage | "settings";

export type SettingsSection =
  | "general"
  | "routing"
  | "network"
  | "data"
  | "about";

export const SETTINGS_SECTIONS: SettingsSection[] = [
  "general",
  "routing",
  "network",
  "data",
  "about",
];

const APP_PAGES: AppPage[] = ["providers"];

const GLOBAL_PAGES: GlobalPage[] = ["usage"];

export const VIEW_STORAGE_KEY = "cc-switch-last-view";
export const APP_STORAGE_KEY = "cc-switch-last-app";

/** 旧版视图名 → 新视图（已删除的页面统一回到供应商页）。 */
const REMOVED_VIEWS = new Set([
  "openclawEnv",
  "openclawTools",
  "openclawAgents",
  "openclawConfig",
  "workspace",
  "hermesMemory",
  "auth",
  "prompts",
  "sessions",
  "apps",
]);

export function isAppPage(view: View): view is AppPage {
  return (APP_PAGES as string[]).includes(view);
}

export function isGlobalPage(view: View): view is GlobalPage {
  return (GLOBAL_PAGES as string[]).includes(view);
}

export function parseView(value: string | null | undefined): View | null {
  if (!value) return null;
  if (REMOVED_VIEWS.has(value)) return "providers";
  if (value === "settings") return "settings";
  if ((APP_PAGES as string[]).includes(value)) return value as AppPage;
  if ((GLOBAL_PAGES as string[]).includes(value)) return value as GlobalPage;
  return null;
}

/** 应用专属页只对它所属的应用成立；换了应用就回到供应商页。 */
export function appPageBelongsTo(_page: AppPage, _app: AppId): boolean {
  return true;
}

export function readStoredView(): View {
  try {
    return parseView(localStorage.getItem(VIEW_STORAGE_KEY)) ?? "providers";
  } catch {
    return "providers";
  }
}

export function storeView(view: View) {
  try {
    localStorage.setItem(VIEW_STORAGE_KEY, view);
  } catch {
    // 存不了就算了：下次启动回到供应商页
  }
}

/**
 * 旧的设置页签名 → 新的设置分组。用量统计、认证已经搬成全局页，由调用方改走全局页。
 */
export function normalizeSettingsSection(
  tab: string | null | undefined,
): SettingsSection {
  switch (tab) {
    case "proxy":
    case "routing":
      return "routing";
    case "advanced":
    case "data":
      return "data";
    case "network":
    case "about":
    case "general":
      return tab;
    default:
      return "general";
  }
}

/** 旧版全局页共用的应用口径；精简后只剩 Claude。 */
export function sharedFeatureAppOf(_app: AppId): AppId {
  return "claude";
}

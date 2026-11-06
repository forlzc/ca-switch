import type { AppId } from "@/lib/api/types";

/** 返回供应商卡片应展示的图标名。 */
export function resolveProviderIcon(
  _appId: AppId,
  icon?: string,
  _iconColor?: string,
): string | undefined {
  return icon?.trim() || undefined;
}

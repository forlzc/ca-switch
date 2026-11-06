import type { TFunction } from "i18next";

export interface UsageProviderLabel {
  label: string;
  /** 旁边已经有「供应商」列时用的短名，当前与 label 相同 */
  shortLabel: string;
  /** 截断时悬停提示里补充的说明 */
  hint?: string;
}

/** 用量面板里供应商名的显示文案；空名显示「未知供应商」。 */
export function getUsageProviderLabel(
  name: string | undefined,
  t: TFunction,
): UsageProviderLabel {
  if (!name) {
    const label = t("usage.unknownProvider");
    return { label, shortLabel: label };
  }
  return { label: name, shortLabel: name };
}

/** 悬停提示：名字本身（截断时看全名），有说明时再附上。 */
export function usageProviderTitle({
  label,
  hint,
}: Pick<UsageProviderLabel, "label" | "hint">) {
  return hint ? `${label}\n${hint}` : label;
}

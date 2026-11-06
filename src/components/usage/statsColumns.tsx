import { useTranslation } from "react-i18next";
import { HelpTip } from "@/components/ui/help-tip";
import { cn } from "@/lib/utils";
import {
  formatFirstTokenSeconds,
  formatTokensPerSecond,
  getAggregateTokensPerSecond,
} from "./format";
import { usageTable } from "./usageTable";

/** 供应商 / 模型统计行共有的速度分子分母与首字中位数（后端只统计满足条件的明细请求）。 */
export interface SpeedTotals {
  speedOutputTokens?: number;
  speedGenerationMs?: number;
  firstTokenMedianMs?: number | null;
}

/** 一行的汇总速度：Σ输出 ÷ Σ生成时间。 */
export function getStatsSpeed(stat: SpeedTotals): string | null {
  return formatTokensPerSecond(
    getAggregateTokensPerSecond(stat.speedOutputTokens, stat.speedGenerationMs),
  );
}

/** 「成功率」「速度」「首字」三列的表头，供应商和模型统计共用；成功率是性能组的第一列，前面多留组间距。 */
export function PerformanceHeaders() {
  const { t } = useTranslation();
  return (
    <>
      <th className={cn(usageTable.th, usageTable.groupGap)}>
        {t("usage.successRate")}
      </th>
      <th className={usageTable.th}>
        <span className="inline-flex items-center gap-0.5">
          {t("usage.speed")}
          <HelpTip title={t("usage.speedSumHelpTitle")} align="end">
            {t("usage.speedSumHelp")}
          </HelpTip>
        </span>
      </th>
      <th className={usageTable.th}>
        <span className="inline-flex items-center gap-0.5">
          {t("usage.firstToken")}
          <HelpTip title={t("usage.firstTokenSumHelpTitle")} align="end">
            {t("usage.firstTokenSumHelp")}
          </HelpTip>
        </span>
      </th>
    </>
  );
}

/** 「成功率」「速度」「首字」三列的单元格。 */
export function PerformanceCells({
  stat,
}: {
  stat: SpeedTotals & { successRate: number };
}) {
  const speed = getStatsSpeed(stat);
  const firstToken = formatFirstTokenSeconds(stat.firstTokenMedianMs);
  return (
    <>
      <td className={cn(usageTable.td, usageTable.groupGap)}>
        {stat.successRate.toFixed(stat.successRate >= 99.95 ? 0 : 1)}%
      </td>
      <td className={cn(usageTable.td, speed == null && usageTable.muted)}>
        {speed == null ? (
          "—"
        ) : (
          <>
            {speed}
            <span className="ms-0.5 text-badge font-normal text-fg-3">
              tok/s
            </span>
          </>
        )}
      </td>
      <td className={cn(usageTable.td, firstToken == null && usageTable.muted)}>
        {firstToken ?? "—"}
      </td>
    </>
  );
}

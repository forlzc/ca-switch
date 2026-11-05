import { useMemo } from "react";
import type { AppId } from "@/lib/api";
import { useProxyStatusQuery } from "@/lib/query/proxy";
import { useUsageSummary } from "@/lib/query/usage";
import { parseFiniteNumber } from "@/components/usage/format";
import type { UsageRangeSelection } from "@/types/usage";
import type { AppMode } from "@/types/proxy";

export interface AppNavStatus {
  mode: AppMode;
  /** 需要处理：路由服务没在运行 */
  alert: boolean;
}

const TODAY: UsageRangeSelection = { preset: "today" };

/**
 * 侧栏需要的状态：固定的路由模式标签和提醒、今日花费与 Token。
 * 精简后只剩 Claude 且只走路由。
 */
export function useSidebarStatus() {
  const { data: proxyStatus } = useProxyStatusQuery();
  const { data: todaySummary } = useUsageSummary(TODAY, undefined, {
    refetchInterval: 60_000,
  });

  const serviceRunning = proxyStatus?.running ?? false;

  const appStatus = useMemo(() => {
    return (_app: AppId): AppNavStatus => ({
      mode: "route",
      alert: proxyStatus !== undefined && !serviceRunning,
    });
  }, [proxyStatus, serviceRunning]);

  const todayCost = parseFiniteNumber(todaySummary?.totalCost) ?? 0;
  // 和用量页「真实消耗 Tokens」同口径
  const todayTokens = todaySummary?.realTotalTokens ?? 0;
  const todayRequests = todaySummary?.totalRequests ?? 0;
  const todayHitRate = todaySummary?.cacheHitRate ?? 0;

  return {
    appStatus,
    todayCost,
    todayTokens,
    todayRequests,
    todayHitRate,
  };
}

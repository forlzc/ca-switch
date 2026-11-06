import { useEffect } from "react";
import { focusManager, useQueryClient } from "@tanstack/react-query";
import { usageKeys } from "@/lib/query/usage";

interface UsageAutoRefreshOptions {
  /** 时间范围随「现在」移动（今天、最近 24 小时等） */
  rollingRange?: boolean;
}

/** 随时间移动的范围多久重算一次：跨过午夜、滚动窗口移出旧记录都靠它 */
const ROLLING_RANGE_REFRESH_MS = 60_000;

/**
 * 范围随时间移动时每分钟重查一次，否则没有新用量时跨过午夜「今天」还停在昨天。
 * 新用量由后端事件触发 useUsageEventBridge 刷新，这里不扫描任何日志。
 */
export function useUsageAutoRefresh({
  rollingRange = false,
}: UsageAutoRefreshOptions = {}) {
  const queryClient = useQueryClient();

  useEffect(() => {
    if (!rollingRange) return;
    const timer = window.setInterval(() => {
      if (focusManager.isFocused()) {
        void queryClient.invalidateQueries({ queryKey: usageKeys.all });
      }
    }, ROLLING_RANGE_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [queryClient, rollingRange]);
}

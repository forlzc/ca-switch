import { useSettings } from "@/hooks/useSettings";
import { useAutoExchangeRates } from "@/hooks/useAutoExchangeRates";
import type { SettingsFormState } from "@/hooks/useSettings";
import { UsageDashboard } from "./UsageDashboard";

interface UsagePageProps {
  /** 打开设置 → 本地路由（「记录请求用量」开关在那里） */
  onOpenRoutingSettings?: () => void;
}

/** 侧栏「用量统计」全局页：数据来自本地代理记录的请求日志。 */
export function UsagePage({ onOpenRoutingSettings }: UsagePageProps = {}) {
  const { settings, autoSaveSettings } = useSettings();

  const save = async (updates: Partial<SettingsFormState>) => {
    try {
      return (await autoSaveSettings(updates)) != null;
    } catch {
      // useSettings 已提示保存失败；将结果传回面板回滚乐观更新。
      return false;
    }
  };

  // 自动模式的汇率：进用量页时过期就取一次，取到后写回设置
  useAutoExchangeRates(
    settings?.usageDisplay,
    (usageDisplay) => save({ usageDisplay }),
    settings != null,
  );

  return (
    <UsageDashboard
      onOpenRoutingSettings={onOpenRoutingSettings}
      usageDisplay={settings?.usageDisplay}
      onUsageDisplayChange={(usageDisplay) => save({ usageDisplay })}
    />
  );
}

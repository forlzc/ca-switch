import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, Loader2 } from "lucide-react";
import {
  Sheet,
  SheetBody,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import { HelpTip } from "@/components/ui/help-tip";
import type { UsageDisplaySettings } from "@/types";
import { useGlobalProxyConfig } from "@/lib/query/proxy";
import { cn } from "@/lib/utils";
import { UsageDisplaySection } from "./UsageDisplaySection";
import { settingsCardClass, settingsGroupTitleClass } from "./settingsStyles";

interface UsageSettingsSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** 打开设置 → 本地路由（「记录请求用量」开关在那里） */
  onOpenRoutingSettings?: () => void;
  /** 金额显示币种、侧栏「今日」、趋势图样式（设备级设置） */
  usageDisplay?: UsageDisplaySettings;
  onUsageDisplayChange?: (
    next: UsageDisplaySettings,
  ) => Promise<boolean> | boolean | void;
  /** 打开定价对话框（抽屉先关掉） */
  onOpenPricing: () => void;
}

/** 抽屉里的一组：小标题 + 内容 */
function SettingsGroup({
  title,
  children,
}: {
  title: string;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2">
      <h3 className={settingsGroupTitleClass}>{title}</h3>
      {children}
    </section>
  );
}

/** 标题行：标题和「?」在左，开关 / 状态 / 按钮在右 */
function RowHeader({
  title,
  help,
  trailing,
}: {
  title: string;
  help: { title: string; body: string };
  trailing?: ReactNode;
}) {
  return (
    <div className="flex min-h-7 items-center gap-3">
      <div className="flex min-w-0 flex-1 items-center gap-0.5">
        <h4 className="m-0 truncate text-strong font-semibold text-fg-1">
          {title}
        </h4>
        <span className="pointer-events-auto flex">
          <HelpTip title={help.title}>{help.body}</HelpTip>
        </span>
      </div>
      {trailing}
    </div>
  );
}

function LoggingStatus() {
  const { t } = useTranslation();
  const { data: config, isLoading } = useGlobalProxyConfig();
  if (isLoading || !config) {
    return (
      <Loader2
        aria-hidden="true"
        className="h-4 w-4 text-fg-3 motion-safe:animate-spin"
      />
    );
  }
  return (
    <span className="text-body text-fg-2">
      {config.enableLogging ? t("usage.sources.on") : t("usage.sources.off")}
    </span>
  );
}

/**
 * 用量页唯一的设置入口：数据来源（路由请求日志）、显示（币种、侧栏「今日」、
 * 趋势图样式）、定价（打开定价对话框）。
 */
export function UsageSettingsSheet({
  open,
  onOpenChange,
  onOpenRoutingSettings,
  usageDisplay,
  onUsageDisplayChange,
  onOpenPricing,
}: UsageSettingsSheetProps) {
  const { t } = useTranslation();

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        width={440}
        closeLabel={t("common.close")}
        dismissOnOutsideClick
      >
        <SheetHeader className="pb-3">
          <SheetTitle>{t("usage.settings.title")}</SheetTitle>
          <SheetDescription className="sr-only">
            {t("usage.settings.title")}
          </SheetDescription>
        </SheetHeader>
        <SheetBody className="flex flex-col gap-6 border-t border-border">
          <SettingsGroup title={t("usage.dataSources")}>
            <div className="divide-y divide-border overflow-hidden rounded-panel border border-border">
              {/* 只读状态 + 去别处修改：整行就是入口（「已开启 ›」），不再单放一个按钮 */}
              <div className="relative">
                {onOpenRoutingSettings && (
                  <button
                    type="button"
                    aria-label={t("usage.sources.editLogging")}
                    className="absolute inset-0 transition-colors hover:bg-subtle focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
                    onClick={() => {
                      onOpenChange(false);
                      onOpenRoutingSettings();
                    }}
                  />
                )}
                <div className="pointer-events-none relative px-4 py-3.5">
                  <RowHeader
                    title={t("routingSettings.logging")}
                    help={{
                      title: t("usage.sources.loggingHelpTitle"),
                      body: t("usage.sources.loggingHelp"),
                    }}
                    trailing={
                      <span className="flex items-center gap-0.5">
                        {open ? <LoggingStatus /> : null}
                        {onOpenRoutingSettings && (
                          <ChevronRight
                            aria-hidden="true"
                            className="h-4 w-4 text-fg-3"
                          />
                        )}
                      </span>
                    }
                  />
                </div>
              </div>
            </div>
          </SettingsGroup>

          {onUsageDisplayChange && (
            <UsageDisplaySection
              value={usageDisplay}
              onChange={onUsageDisplayChange}
            />
          )}

          <SettingsGroup title={t("usage.tabs.pricing")}>
            <div className={cn(settingsCardClass, "flex-row items-center")}>
              <p className="m-0 min-w-0 flex-1 text-caption text-fg-2">
                {t("usage.settings.pricingSummary")}
              </p>
              <Button
                type="button"
                variant="neutral"
                size="compact"
                onClick={() => {
                  onOpenChange(false);
                  onOpenPricing();
                }}
              >
                {t("usage.settings.managePricing")}
              </Button>
            </div>
          </SettingsGroup>
        </SheetBody>
      </SheetContent>
    </Sheet>
  );
}

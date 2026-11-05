import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "@/lib/toast";
import type { Provider } from "@/types";
import { proxyApi } from "@/lib/api/proxy";
import { proxyKeys, useProxyStatusQuery } from "@/lib/query/proxy";
import { ProviderList } from "@/components/providers/ProviderList";
import type { SwitchModeInput } from "@/components/providers/presentation";
import { Notice } from "@/components/ui/notice";
import { Button } from "@/components/ui/button";

type ListCallbacks = Pick<
  React.ComponentProps<typeof ProviderList>,
  | "onEdit"
  | "onDelete"
  | "onDuplicate"
  | "onConfigureUsage"
  | "onOpenWebsite"
  | "onOpenTerminal"
  | "onCreate"
  | "searchOpen"
  | "onSearchOpenChange"
>;

interface CustomProviderPanelProps extends ListCallbacks {
  providers: Record<string, Provider>;
  currentProviderId: string;
  isLoading: boolean;
  scrollRef?: React.Ref<HTMLDivElement>;
  onSwitch: (provider: Provider) => void;
}

/**
 * 精简后的供应商页：没有直连 / 聚合，只走路由。列表按「路由中 / 路由到这里」呈现，
 * 切换供应商就是把路由指针指过去。
 */
export function CustomProviderPanel({
  providers,
  currentProviderId,
  isLoading,
  scrollRef,
  onSwitch,
  ...listCallbacks
}: CustomProviderPanelProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: proxyStatus } = useProxyStatusQuery();
  const serviceRunning = proxyStatus?.running ?? false;
  const [starting, setStarting] = useState(false);

  const switchMode = useMemo<SwitchModeInput>(
    () => ({
      app: "claude",
      t,
      providers: Object.values(providers),
      active: "route",
      view: "route",
      directId: null,
      routeId: currentProviderId || null,
      failoverOn: false,
      queue: [],
      stackMembers: new Map(),
      serviceRunning,
      routingReason: () => "",
      actions: {
        switchDirect: () => undefined,
        needsRouteDialog: () => undefined,
        exitAndUse: () => undefined,
        routeTo: onSwitch,
        queueAdd: () => undefined,
        queueRemove: () => undefined,
        queueMove: () => undefined,
        stackAdd: () => undefined,
        stackRemove: () => undefined,
        stackSetDefault: () => undefined,
      },
    }),
    [t, providers, currentProviderId, serviceRunning, onSwitch],
  );

  const startService = async () => {
    setStarting(true);
    try {
      await proxyApi.startProxyServer();
      await queryClient.invalidateQueries({ queryKey: proxyKeys.status });
    } catch (error) {
      toast.error(
        t("proxy.server.startFailed", {
          detail: String(error) || t("common.unknown"),
        }),
      );
    } finally {
      setStarting(false);
    }
  };

  return (
    <div
      ref={scrollRef}
      id="main-content"
      className="min-h-0 flex-1 overflow-y-auto scroll-stable overflow-x-hidden px-6 pb-12 pt-4"
    >
      {proxyStatus !== undefined && !serviceRunning && (
        <div role="status" className="mb-3">
          <Notice
            tone="warning"
            title={t("mode.notice.serviceDown", {
              app: t("apps.claude"),
              mode: t("mode.names.route"),
            })}
            actions={
              <Button
                variant="neutral"
                size="compact"
                disabled={starting}
                onClick={() => void startService()}
              >
                {t("mode.notice.startService")}
              </Button>
            }
          />
        </div>
      )}
      <ProviderList
        {...listCallbacks}
        providers={providers}
        currentProviderId={currentProviderId}
        appId="claude"
        isLoading={isLoading}
        onSwitch={onSwitch}
        switchMode={switchMode}
      />
    </div>
  );
}

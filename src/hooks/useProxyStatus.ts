/**
 * 代理服务状态管理 Hook
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "@/lib/toast";
import { useTranslation } from "react-i18next";
import { proxyApi } from "@/lib/api/proxy";
import { proxyKeys, useProxyStatusQuery } from "@/lib/query/proxy";
import { extractErrorMessage } from "@/utils/errorUtils";

/**
 * 代理服务状态管理
 */
export function useProxyStatus() {
  const queryClient = useQueryClient();
  const { t } = useTranslation();

  // 查询状态（自动轮询）
  const { data: status, isPending: isProxyStatusPending } =
    useProxyStatusQuery();

  // 启动服务器
  const startProxyServerMutation = useMutation({
    mutationFn: () => proxyApi.startProxyServer(),
    onSuccess: (info) => {
      toast.success(
        t("proxy.server.started", {
          address: info.address,
          port: info.port,
          defaultValue: `代理服务已启动 - ${info.address}:${info.port}`,
        }),
        { closeButton: true },
      );
      queryClient.invalidateQueries({ queryKey: proxyKeys.status });
    },
    onError: (error: Error) => {
      const detail =
        extractErrorMessage(error) ||
        t("common.unknown", { defaultValue: "未知错误" });
      toast.error(
        t("proxy.server.startFailed", {
          detail,
          defaultValue: `启动代理服务失败: ${detail}`,
        }),
      );
    },
  });

  // 停止服务器
  const stopProxyServerMutation = useMutation({
    mutationFn: () => proxyApi.stopProxyServer(),
    onSuccess: () => {
      toast.success(
        t("proxy.server.stopped", {
          defaultValue: "代理服务已停止",
        }),
        { closeButton: true },
      );
      queryClient.invalidateQueries({ queryKey: proxyKeys.status });
    },
    onError: (error: Error) => {
      const detail =
        extractErrorMessage(error) ||
        t("common.unknown", { defaultValue: "未知错误" });
      toast.error(
        t("proxy.server.stopFailed", {
          detail,
          defaultValue: `停止代理服务失败: ${detail}`,
        }),
      );
    },
  });

  return {
    status,
    isRunning: status?.running || false,
    isInitialStatusPending: isProxyStatusPending,

    // 启动/停止
    startProxyServer: startProxyServerMutation.mutateAsync,
    stopProxyServer: stopProxyServerMutation.mutateAsync,

    // 加载状态
    isStarting: startProxyServerMutation.isPending,
    isStoppingServer: stopProxyServerMutation.isPending,
    isPending:
      startProxyServerMutation.isPending || stopProxyServerMutation.isPending,
  };
}

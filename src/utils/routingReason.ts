import type { TFunction } from "i18next";
import type { AppId } from "@/lib/api";
import type { Provider } from "@/types";
import {
  extractCodexWireApi,
  isCodexAnthropicWireApi,
  isCodexChatWireApi,
} from "@/utils/providerConfigUtils";
import { providerNeedsRouting } from "@/utils/providerCapabilities";

/**
 * 这家为什么需要路由（「需要路由」徽标的说明、直连切换时的提示、对话框 F 的正文都用它）。
 * 不需要路由时返回 null。只按协议判断，不区分具体客户端。
 */
export function getRoutingReason(
  app: AppId,
  provider: Provider,
  t: TFunction,
): string | null {
  if (!providerNeedsRouting(app, provider)) {
    return null;
  }

  const config = provider.settingsConfig as Record<string, any> | undefined;
  const wireApi =
    typeof config?.config === "string"
      ? extractCodexWireApi(config.config)
      : undefined;
  const apiFormat = provider.meta?.apiFormat;

  if (apiFormat === "openai_chat" || isCodexChatWireApi(wireApi)) {
    return t("notifications.proxyReasonOpenAIChat", {
      defaultValue: "使用 OpenAI Chat 接口格式",
    });
  }

  if (apiFormat === "openai_responses") {
    return t("notifications.proxyReasonOpenAIResponses", {
      defaultValue: "使用 OpenAI Responses 接口格式",
    });
  }

  if (apiFormat === "anthropic" || isCodexAnthropicWireApi(wireApi)) {
    return t("notifications.proxyReasonAnthropicMessages", {
      defaultValue: "使用 Anthropic Messages 接口格式",
    });
  }

  if (provider.meta?.isFullUrl) {
    return t("notifications.proxyReasonFullUrl", {
      defaultValue: "开启了完整 URL 连接模式",
    });
  }

  return t("notifications.proxyReasonRoutingRequired", {
    defaultValue: "需要本地路由处理请求",
  });
}

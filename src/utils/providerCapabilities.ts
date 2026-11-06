import type { AppId } from "@/lib/api";
import type { Provider } from "@/types";
import { isOAuthProviderType } from "@/config/constants";

/**
 * 官方账号卡：上游是厂商自己的账号体系，不走本地代理转发。
 */
export function isOfficialAccount(
  _appId: AppId,
  provider: Pick<Provider, "category">,
): boolean {
  return provider.category === "official";
}

/**
 * 能进故障转移队列：官方账号卡不能和别家轮换。
 */
export function supportsFailover(
  appId: AppId,
  provider: Pick<Provider, "category">,
): boolean {
  return !isOfficialAccount(appId, provider);
}

/**
 * 能否做「检测连通」（对 base_url 的可达性探测）。
 * 官方账号卡的 base_url 故意留空，不参与探测。
 */
export function supportsConnectivityCheck(
  _appId: AppId,
  provider: Pick<Provider, "category">,
): boolean {
  return (
    provider.category !== "official" &&
    provider.category !== "omo" &&
    provider.category !== "omo-slim"
  );
}

/** 官方账号卡不允许走本地路由接管。 */
export function supportsOfficialProxyTakeover(
  _appId: AppId,
  _provider: Pick<Provider, "id" | "category" | "meta" | "settingsConfig">,
): boolean {
  return false;
}

/**
 * 供应商是否必须开启路由接管才能正常工作（badge 与切换警告共用的权威谓词）。
 *
 * - 官方供应商：直连即可，不需要代理。
 * - 托管 OAuth 供应商：凭据由本地代理按请求注入，必须接管。
 * - 其余：完整 URL 或非 Anthropic 格式需要本地代理转换。
 */
export function providerNeedsRouting(
  appId: AppId,
  provider: Provider,
): boolean {
  if (isOfficialAccount(appId, provider)) return false;

  const isManagedOAuth = isOAuthProviderType(provider.meta?.providerType);
  if (isManagedOAuth) return true;

  const fmt = provider.meta?.apiFormat;
  return provider.meta?.isFullUrl === true || (!!fmt && fmt !== "anthropic");
}

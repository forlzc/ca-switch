import { invoke } from "@tauri-apps/api/core";
import type {
  RouteSwitchResult,
  ProxyStatus,
  ProxyServerInfo,
  ProxyTakeoverStatus,
  GlobalProxyConfig,
  AppProxyConfig,
} from "@/types/proxy";

export const proxyApi = {
  // ========== 代理服务器控制 API ==========

  // 启动代理服务器
  async startProxyServer(): Promise<ProxyServerInfo> {
    return invoke("start_proxy_server");
  },

  // 停止代理服务器（不恢复已接管配置）
  async stopProxyServer(): Promise<void> {
    return invoke("stop_proxy_server");
  },

  // 停止代理服务器并恢复配置
  async stopProxyWithRestore(): Promise<void> {
    return invoke("stop_proxy_with_restore");
  },

  // 获取代理服务器状态
  async getProxyStatus(): Promise<ProxyStatus> {
    return invoke("get_proxy_status");
  },

  // ========== 接管状态 API ==========

  // 获取各应用接管状态
  async getProxyTakeoverStatus(): Promise<ProxyTakeoverStatus> {
    return invoke("get_proxy_takeover_status");
  },

  // 为指定应用进入/退出代理模式。stack 为真时进入的是 Stack 模式（和路由模式二选一）；
  // route 是确认框里选的路由目标（Stack 模式下是默认那家），不传沿用上次的路由
  async setProxyTakeoverForApp(
    appType: string,
    enabled: boolean,
    stack = false,
    route?: string | null,
  ): Promise<void> {
    return invoke("set_proxy_takeover_for_app", {
      appType,
      enabled,
      stack,
      route: route ?? null,
    });
  },

  // 指定路由目标（聚合模式下是默认那家）：直连时只记下来、下次进入路由 / 聚合模式时用它，
  // 已经在路由 / 聚合模式时当场生效
  async setProxyRoute(
    appType: string,
    providerId: string,
  ): Promise<RouteSwitchResult> {
    return invoke("set_proxy_route", { appType, providerId });
  },

  // ========== v3+ 全局/应用级配置 API ==========

  // 获取全局代理配置
  async getGlobalProxyConfig(): Promise<GlobalProxyConfig> {
    return invoke("get_global_proxy_config");
  },

  // 更新全局代理配置
  async updateGlobalProxyConfig(config: GlobalProxyConfig): Promise<void> {
    return invoke("update_global_proxy_config", { config });
  },

  // 获取客户端地址（写给客户端的代理主机，空表示按监听地址推）
  async getProxyClientHost(): Promise<string> {
    return invoke("get_proxy_client_host");
  },

  // 保存客户端地址，并按新地址重写接上路由的客户端
  async setProxyClientHost(host: string): Promise<void> {
    return invoke("set_proxy_client_host", { host });
  },

  // 获取指定应用的代理配置
  async getProxyConfigForApp(appType: string): Promise<AppProxyConfig> {
    return invoke("get_proxy_config_for_app", { appType });
  },

  // 更新指定应用的代理配置
  async updateProxyConfigForApp(config: AppProxyConfig): Promise<void> {
    return invoke("update_proxy_config_for_app", { config });
  },

  // ========== 计费默认配置 API ==========

  // 获取计费模式来源
  async getPricingModelSource(appType: string): Promise<string> {
    return invoke("get_pricing_model_source", { appType });
  },

  // 设置计费模式来源
  async setPricingModelSource(appType: string, value: string): Promise<void> {
    return invoke("set_pricing_model_source", { appType, value });
  },
};

import type { TFunction } from "i18next";
import type { RouteSwitchResult } from "@/types/proxy";

export function routeSwitchNotification(
  result: RouteSwitchResult,
  _app: string,
  provider: string,
  t: TFunction,
): string {
  switch (result.kind) {
    case "remembered":
      return t("mode.toast.routeRemembered", {
        provider,
        defaultValue: "已记住供应商 {{provider}}，后续请求将使用该供应商。",
      });
    case "route":
      return result.clientConfigChanged
        ? t("mode.toast.routeTargetChanged", {
            provider,
            defaultValue: "路由目标已设为 {{provider}}。",
          })
        : t("mode.toast.routeSwitched", {
            provider,
            defaultValue:
              "路由已切换至 {{provider}}，后续新请求将使用该供应商。",
          });
    case "stack":
      return t("mode.toast.stackDefaultChanged", {
        provider,
        defaultValue:
          "聚合默认供应商已设为 {{provider}}；已选聚合模型仍使用各自的供应商。",
      });
  }
}

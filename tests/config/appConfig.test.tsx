import { describe, expect, it } from "vitest";
import {
  APP_IDS,
  orderAppIds,
  isProxyAppId,
  isModeAppId,
} from "@/config/appConfig";

describe("appConfig（精简后只保留 Claude）", () => {
  it("只列出 Claude", () => {
    expect(APP_IDS).toEqual(["claude"]);
    expect(orderAppIds(undefined)).toEqual(["claude"]);
  });

  it("Claude 是路由应用", () => {
    expect(isProxyAppId("claude")).toBe(true);
    expect(isModeAppId("claude")).toBe(true);
  });
});

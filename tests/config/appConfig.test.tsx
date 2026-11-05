import { describe, expect, it } from "vitest";
import {
  APP_IDS,
  orderAppIds,
  isAdditiveAppId,
  MCP_APP_IDS,
  isProxyAppId,
  isModeAppId,
} from "@/config/appConfig";

describe("appConfig（精简后只保留 Claude）", () => {
  it("只列出 Claude", () => {
    expect(APP_IDS).toEqual(["claude"]);
    expect(orderAppIds(undefined)).toEqual(["claude"]);
  });

  it("Claude 是路由应用，不再是累加式应用", () => {
    expect(isProxyAppId("claude")).toBe(true);
    expect(isModeAppId("claude")).toBe(true);
    expect(isAdditiveAppId("claude")).toBe(false);
    expect(isAdditiveAppId("codex")).toBe(false);
    expect(MCP_APP_IDS).toHaveLength(0);
  });
});

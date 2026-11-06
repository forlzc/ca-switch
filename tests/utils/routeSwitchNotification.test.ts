import { describe, expect, it } from "vitest";
import { createInstance } from "i18next";
import { routeSwitchNotification } from "@/utils/routeSwitchNotification";
import type { RouteSwitchResult } from "@/types/proxy";
import en from "@/i18n/locales/en.json";
import zh from "@/i18n/locales/zh.json";
import tw from "@/i18n/locales/zh-TW.json";
import ja from "@/i18n/locales/ja.json";
import tr from "@/i18n/locales/tr.json";

async function translate(locale = "zh") {
  const i18n = createInstance();
  await i18n.init({
    lng: locale,
    resources: {
      en: { translation: en },
      zh: { translation: zh },
      "zh-TW": { translation: tw },
      ja: { translation: ja },
      tr: { translation: tr },
    },
    interpolation: { escapeValue: false },
  });
  return i18n.t;
}

describe("route switch notifications", () => {
  it("reports the switched provider without a restart hint", async () => {
    const t = await translate();
    expect(
      routeSwitchNotification(
        { kind: "route", clientConfigChanged: false },
        "claude",
        "B",
        t,
      ),
    ).toBe("路由已切换至 B，后续新请求将使用该供应商。");
  });

  it("reports a changed routing target", async () => {
    const t = await translate();
    expect(
      routeSwitchNotification(
        { kind: "route", clientConfigChanged: true },
        "claude",
        "B",
        t,
      ),
    ).toBe("路由目标已设为 B。");
  });

  it("reports a remembered choice without claiming it is routing", async () => {
    const t = await translate();
    expect(
      routeSwitchNotification(
        { kind: "remembered", clientConfigChanged: false },
        "claude",
        "B",
        t,
      ),
    ).toBe("已记住供应商 B，客户端接入路由或聚合后使用。");
  });

  it.each(["en", "zh", "zh-TW", "ja", "tr"])(
    "has complete translations and interpolation for %s",
    async (locale) => {
      const t = await translate(locale);
      for (const kind of [
        "remembered",
        "route",
        "stack",
      ] as RouteSwitchResult["kind"][]) {
        const message = routeSwitchNotification(
          { kind, clientConfigChanged: kind !== "remembered" },
          "claude",
          "Provider B",
          t,
        );
        expect(message).toContain("Provider B");
        expect(message).not.toMatch(/mode\.toast|\{\{|undefined/);
      }
    },
  );
});

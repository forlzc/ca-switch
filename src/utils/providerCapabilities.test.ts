import { describe, expect, it } from "vitest";
import {
  isOfficialAccount,
  providerNeedsRouting,
  supportsConnectivityCheck,
  supportsFailover,
  supportsOfficialProxyTakeover,
} from "@/utils/providerCapabilities";
import type { Provider } from "@/types";

function provider(overrides: Partial<Provider> = {}): Provider {
  return {
    id: "p1",
    name: "P1",
    category: "custom",
    settingsConfig: { env: {} },
    ...overrides,
  } as Provider;
}

describe("providerCapabilities", () => {
  it("treats official category as an official account", () => {
    expect(isOfficialAccount("claude", provider({ category: "official" }))).toBe(
      true,
    );
    expect(isOfficialAccount("claude", provider())).toBe(false);
  });

  it("keeps official accounts out of failover", () => {
    expect(supportsFailover("claude", provider({ category: "official" }))).toBe(
      false,
    );
    expect(supportsFailover("claude", provider())).toBe(true);
  });

  it("does not probe official accounts", () => {
    expect(
      supportsConnectivityCheck("claude", provider({ category: "official" })),
    ).toBe(false);
    expect(supportsConnectivityCheck("claude", provider())).toBe(true);
  });

  it("never lets official accounts use proxy takeover", () => {
    expect(
      supportsOfficialProxyTakeover("claude", provider({ category: "official" })),
    ).toBe(false);
  });

  it("does not route official or plain Anthropic providers", () => {
    expect(
      providerNeedsRouting("claude", provider({ category: "official" })),
    ).toBe(false);
    expect(
      providerNeedsRouting(
        "claude",
        provider({ meta: { apiFormat: "anthropic" } }),
      ),
    ).toBe(false);
  });

  it("routes providers that need protocol conversion", () => {
    expect(
      providerNeedsRouting(
        "claude",
        provider({ meta: { apiFormat: "openai_chat" } }),
      ),
    ).toBe(true);
    expect(
      providerNeedsRouting(
        "claude",
        provider({ meta: { apiFormat: "anthropic", isFullUrl: true } }),
      ),
    ).toBe(true);
  });
});

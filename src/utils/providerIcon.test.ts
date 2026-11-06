import { describe, expect, it } from "vitest";
import { resolveProviderIcon } from "./providerIcon";

describe("resolveProviderIcon", () => {
  it("returns the trimmed icon name", () => {
    expect(resolveProviderIcon("claude", "grok", "")).toBe("grok");
  });

  it("normalizes an empty icon to the initials fallback", () => {
    expect(resolveProviderIcon("claude", "  ", "")).toBeUndefined();
    expect(resolveProviderIcon("claude")).toBeUndefined();
  });
});

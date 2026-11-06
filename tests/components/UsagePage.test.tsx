import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { UsagePage } from "@/components/usage/UsagePage";
import { UsageDashboard } from "@/components/usage/UsageDashboard";

const autoSaveSettings = vi.hoisted(() => vi.fn());
const settingsState = vi.hoisted(() => ({
  current: null as null | Record<string, unknown>,
}));
vi.mock("@/hooks/useSettings", () => ({
  useSettings: () => ({ settings: settingsState.current, autoSaveSettings }),
}));
vi.mock("@/hooks/useAutoExchangeRates", () => ({
  useAutoExchangeRates: vi.fn(),
}));
vi.mock("@/components/usage/UsageDashboard", () => ({
  UsageDashboard: vi.fn(() => null),
}));

describe("UsagePage settings persistence", () => {
  beforeEach(() => {
    autoSaveSettings.mockReset();
    vi.mocked(UsageDashboard).mockClear();
    settingsState.current = {
      usageDisplay: { currency: "USD" },
    };
  });

  it("passes display settings through to the dashboard", () => {
    render(<UsagePage />);
    expect(vi.mocked(UsageDashboard).mock.lastCall![0]).toMatchObject({
      usageDisplay: { currency: "USD" },
    });
  });

  it("waits for saving before reporting success to the dashboard", async () => {
    let finish!: (value: { requiresRestart: boolean }) => void;
    autoSaveSettings.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    render(<UsagePage />);
    const onChange =
      vi.mocked(UsageDashboard).mock.calls[0][0]!.onUsageDisplayChange!;
    const settled = vi.fn();
    const save = Promise.resolve(onChange({ currency: "CNY" })).then(settled);
    await Promise.resolve();
    expect(settled).not.toHaveBeenCalled();
    expect(autoSaveSettings).toHaveBeenCalledWith({
      usageDisplay: { currency: "CNY" },
    });
    finish({ requiresRestart: false });
    await save;
    expect(settled).toHaveBeenCalledWith(true);
  });

  it.each(["rejected", "not loaded"])(
    "reports a failed save when settings are %s",
    async (failure) => {
      if (failure === "rejected")
        autoSaveSettings.mockRejectedValue(new Error("save failed"));
      else autoSaveSettings.mockResolvedValue(null);
      render(<UsagePage />);
      const props = vi.mocked(UsageDashboard).mock.calls[0][0]!;
      await expect(props.onUsageDisplayChange!({ currency: "CNY" })).resolves.toBe(
        false,
      );
    },
  );
});

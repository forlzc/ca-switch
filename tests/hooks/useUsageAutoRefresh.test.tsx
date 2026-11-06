import type { PropsWithChildren } from "react";
import { act, cleanup, renderHook } from "@testing-library/react";
import {
  focusManager,
  QueryClient,
  QueryClientProvider,
} from "@tanstack/react-query";
import {
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
  type MockInstance,
} from "vitest";
import { useUsageAutoRefresh } from "@/hooks/useUsageAutoRefresh";
import { usageKeys } from "@/lib/query/usage";

const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));

describe("useUsageAutoRefresh", () => {
  let client: QueryClient;
  let invalidate: MockInstance<QueryClient["invalidateQueries"]>;

  function wrapper({ children }: PropsWithChildren) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
  }

  beforeEach(() => {
    vi.useFakeTimers();
    focusManager.setFocused(true);
    client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    invalidate = vi.spyOn(client, "invalidateQueries");
  });

  afterEach(() => {
    cleanup();
    client.clear();
    focusManager.setFocused(undefined);
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("re-runs usage queries every minute for rolling ranges", async () => {
    renderHook(() => useUsageAutoRefresh({ rollingRange: true }), { wrapper });
    await advance(59_999);
    expect(invalidate).not.toHaveBeenCalled();
    await advance(1);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: usageKeys.all });
  });

  it("does not re-run usage queries for fixed ranges", async () => {
    renderHook(() => useUsageAutoRefresh({ rollingRange: false }), {
      wrapper,
    });
    await advance(120_000);
    expect(invalidate).not.toHaveBeenCalled();
  });

  it("pauses while the window is hidden", async () => {
    focusManager.setFocused(false);
    renderHook(() => useUsageAutoRefresh({ rollingRange: true }), { wrapper });
    await advance(60_000);
    expect(invalidate).not.toHaveBeenCalled();
    act(() => focusManager.setFocused(true));
    await advance(60_000);
    expect(invalidate).toHaveBeenCalledTimes(1);
  });
});

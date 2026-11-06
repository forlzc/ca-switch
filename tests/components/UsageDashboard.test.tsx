import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { UsageDashboard } from "@/components/usage/UsageDashboard";
import type { UsageRangeSelection } from "@/types/usage";

const useProviderStatsMock = vi.hoisted(() => vi.fn());
const useModelStatsMock = vi.hoisted(() => vi.fn());
const useSummaryByAppMock = vi.hoisted(() => vi.fn());
const usageHeroMock = vi.hoisted(() => vi.fn());
const requestLogTableMock = vi.hoisted(() => vi.fn());
const detailPanelMock = vi.hoisted(() => vi.fn());
const usageApiMock = vi.hoisted(() => ({
  getUsageSummary: vi.fn(),
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: unknown) => {
      if (typeof options === "string") return options;
      if (options && typeof options === "object") {
        const values = Object.entries(options as Record<string, unknown>)
          .filter(([name]) => name !== "defaultValue")
          .map(([, value]) => String(value));
        return values.length ? `${key}:${values.join(",")}` : key;
      }
      return key;
    },
    i18n: {
      resolvedLanguage: "en",
      language: "en",
    },
  }),
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() },
}));

vi.mock("@/hooks/useUsageEventBridge", () => ({
  useUsageEventBridge: () => {},
}));

vi.mock("@/lib/api/usage", () => ({ usageApi: usageApiMock }));

vi.mock("@/lib/query/usage", async () => {
  const actual =
    await vi.importActual<typeof import("@/lib/query/usage")>(
      "@/lib/query/usage",
    );
  return {
    ...actual,
    useProviderStats: (...args: unknown[]) => useProviderStatsMock(...args),
    useModelStats: (...args: unknown[]) => useModelStatsMock(...args),
    useUsageSummaryByApp: (...args: unknown[]) => useSummaryByAppMock(...args),
  };
});

vi.mock("@/components/usage/UsageHero", () => ({
  UsageHero: (props: unknown) => {
    usageHeroMock(props);
    return <div data-testid="usage-hero" />;
  },
}));

vi.mock("@/components/usage/UsageTrendChart", () => ({
  UsageTrendChart: () => <div data-testid="usage-trend" />,
}));

vi.mock("@/components/usage/UsageHeatmap", () => ({
  UsageHeatmap: () => <div data-testid="usage-heatmap" />,
}));

vi.mock("@/components/usage/RequestLogTable", () => ({
  RequestLogTable: (props: { onOpenDetail?: (id: string) => void }) => {
    requestLogTableMock(props);
    return (
      <button type="button" onClick={() => props.onOpenDetail?.("req-1")}>
        open-row
      </button>
    );
  },
}));

vi.mock("@/components/usage/RequestDetailPanel", () => ({
  RequestDetailPanel: (props: { requestId: string | null }) => {
    detailPanelMock(props);
    return props.requestId ? (
      <div data-testid="request-detail">{props.requestId}</div>
    ) : null;
  },
}));

vi.mock("@/components/usage/ProviderStatsTable", () => ({
  ProviderStatsTable: () => <div data-testid="provider-stats-table" />,
}));

vi.mock("@/components/usage/ModelStatsTable", () => ({
  ModelStatsTable: () => <div data-testid="model-stats-table" />,
}));

vi.mock("@/components/usage/PricingConfigPanel", () => ({
  PricingConfigPanel: () => <div data-testid="pricing-config-panel" />,
}));

vi.mock("@/components/usage/UsageDateRangePicker", () => ({
  UsageDateRangePicker: ({
    triggerLabel,
    onApply,
  }: {
    triggerLabel: string;
    onApply: (selection: UsageRangeSelection) => void;
  }) => (
    <>
      <button type="button">{triggerLabel}</button>
      <button type="button" onClick={() => onApply({ preset: "all" })}>
        pick-all
      </button>
      <button
        type="button"
        onClick={() =>
          onApply({
            preset: "custom",
            customStartDate: 1_700_000_000,
            customEndDate: 1_700_086_400,
          })
        }
      >
        pick-custom
      </button>
    </>
  ),
}));

vi.mock("@/lib/query/proxy", () => ({
  useGlobalProxyConfig: () => ({
    data: { enableLogging: true },
    isLoading: false,
  }),
}));

const renderDashboard = (props: ComponentProps<typeof UsageDashboard> = {}) => {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <UsageDashboard {...props} />
    </QueryClientProvider>,
  );
};

const openSettings = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(
    screen.getByRole("button", { name: "usage.settings.title" }),
  );
};

describe("UsageDashboard", () => {
  beforeEach(() => {
    useProviderStatsMock.mockReset();
    useModelStatsMock.mockReset();
    useSummaryByAppMock.mockReset();
    usageHeroMock.mockReset();
    requestLogTableMock.mockReset();
    detailPanelMock.mockReset();
    useProviderStatsMock.mockReturnValue({
      data: [
        {
          providerId: "p1",
          appType: "claude",
          providerName: "DeepSeek",
          requestCount: 12,
          totalTokens: 100,
          totalCost: "1",
          successRate: 100,
          avgLatencyMs: 0,
        },
      ],
    });
    useModelStatsMock.mockReturnValue({ data: [] });
    useSummaryByAppMock.mockReturnValue({ data: [] });
    usageApiMock.getUsageSummary.mockResolvedValue({ totalRequests: 5 });
  });

  it("swaps the trend chart for the heatmap on the all-time range", async () => {
    const user = userEvent.setup();
    renderDashboard();

    expect(await screen.findByTestId("usage-trend")).toBeInTheDocument();
    expect(screen.queryByTestId("usage-heatmap")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "pick-all" }));

    expect(await screen.findByTestId("usage-heatmap")).toBeInTheDocument();
    expect(screen.queryByTestId("usage-trend")).not.toBeInTheDocument();
    expect(usageHeroMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ range: { preset: "all" } }),
    );
  });

  it("reopens on the last tab and preset range, but not on a custom range", async () => {
    const user = userEvent.setup();
    const first = renderDashboard();

    await user.click(screen.getByRole("button", { name: "pick-all" }));
    await user.click(screen.getByRole("tab", { name: /^usage\.tabs\.models/ }));
    first.unmount();

    const second = renderDashboard();
    expect(await screen.findByTestId("usage-heatmap")).toBeInTheDocument();
    expect(
      screen.getByRole("tab", { name: /^usage\.tabs\.models/ }),
    ).toHaveAttribute("aria-selected", "true");

    // 自定义范围不记：下次还是上一个预设
    await user.click(screen.getByRole("button", { name: "pick-custom" }));
    expect(usageHeroMock).toHaveBeenLastCalledWith(
      expect.objectContaining({
        range: expect.objectContaining({ preset: "custom" }),
      }),
    );
    second.unmount();

    renderDashboard();
    expect(usageHeroMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ range: { preset: "all" } }),
    );
  });

  it("ignores a remembered tab or range it does not recognise", () => {
    // 「设置」页签已经并进用量设置抽屉
    localStorage.setItem("cc-switch.usage.tab", "settings");
    localStorage.setItem("cc-switch.usage.rangePreset", "custom");
    renderDashboard();

    expect(
      screen.getByRole("tab", { name: /^usage\.requestLogs/ }),
    ).toHaveAttribute("aria-selected", "true");
    expect(usageHeroMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ range: { preset: "today" } }),
    );
  });

  it("defaults to today", () => {
    renderDashboard();

    expect(usageHeroMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ metric: "tokens" }),
    );
    expect(
      screen.getByRole("button", { name: "usage.presetToday" }),
    ).toBeInTheDocument();
    expect(usageHeroMock).toHaveBeenLastCalledWith(
      expect.objectContaining({ range: { preset: "today" } }),
    );
  });

  const scopeTrigger = () =>
    screen.getByRole("button", {
      name: /^(usage\.providerFilter\.label|DeepSeek) \/ /,
    });

  it("filters by provider and lists request counts", async () => {
    const user = userEvent.setup();
    renderDashboard();

    await user.click(scopeTrigger());
    const item = await screen.findByRole("menuitem", { name: /DeepSeek/ });
    expect(item).toHaveTextContent("12");
    await user.click(item);

    await waitFor(() =>
      expect(usageHeroMock).toHaveBeenLastCalledWith(
        expect.objectContaining({ providerName: "DeepSeek", model: undefined }),
      ),
    );
    expect(scopeTrigger()).toHaveTextContent(
      "DeepSeek / usage.modelFilter.label",
    );
  });

  it("picks a model from the provider's submenu", async () => {
    const user = userEvent.setup();
    useModelStatsMock.mockImplementation(
      (_range: unknown, filters: { providerName?: string }) => ({
        data:
          filters.providerName === "DeepSeek"
            ? [{ model: "deepseek-chat", requestCount: 7 }]
            : [],
      }),
    );
    renderDashboard();

    await user.click(scopeTrigger());
    const provider = await screen.findByRole("menuitem", { name: /DeepSeek/ });
    provider.focus();
    await user.keyboard("{ArrowRight}");
    await user.click(
      await screen.findByRole("menuitem", { name: /deepseek-chat/ }),
    );

    await waitFor(() =>
      expect(usageHeroMock).toHaveBeenLastCalledWith(
        expect.objectContaining({
          providerName: "DeepSeek",
          model: "deepseek-chat",
        }),
      ),
    );
    expect(scopeTrigger()).toHaveTextContent("DeepSeek / deepseek-chat");
  });

  it("opens the request detail drawer from a log row", async () => {
    const user = userEvent.setup();
    renderDashboard();

    await user.click(screen.getByRole("button", { name: "open-row" }));

    expect(await screen.findByTestId("request-detail")).toHaveTextContent(
      "req-1",
    );
  });

  it("switches tabs and filters logs by status code", async () => {
    const user = userEvent.setup();
    renderDashboard({ onUsageDisplayChange: vi.fn() });

    await user.click(
      screen.getByRole("button", {
        name: "usage.statusCode: usage.statusFilter.all",
      }),
    );
    await user.click(await screen.findByRole("menuitem", { name: "429" }));
    await waitFor(() =>
      expect(requestLogTableMock).toHaveBeenLastCalledWith(
        expect.objectContaining({ statusCode: 429 }),
      ),
    );

    // 状态筛选和「显示的列」只在请求日志页签的工具栏里
    expect(
      screen.getByRole("button", { name: "usage.display.logColumns" }),
    ).toBeInTheDocument();
    await user.click(
      screen.getByRole("tab", { name: /^usage\.tabs\.providers/ }),
    );
    expect(screen.getByTestId("provider-stats-table")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /^usage\.statusCode/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("tab", { name: "usage.tabs.settings" }),
    ).not.toBeInTheDocument();
  });

  it("draws the request log with the columns picked in settings", () => {
    renderDashboard({
      usageDisplay: { currency: "CNY", requestLogColumns: {} },
    });

    // 改过的列按设置走，没改过的按默认（新列默认不显示），时间列总在
    const columns = requestLogTableMock.mock.lastCall?.[0].columns;
    expect(columns).toContain("time");
    expect(columns).not.toContain("statusCode");
  });

  it("puts display settings above the pricing entry in the settings drawer", async () => {
    const user = userEvent.setup();
    renderDashboard({
      usageDisplay: { currency: "CNY" },
      onUsageDisplayChange: vi.fn(),
    });

    await openSettings(user);
    const display = await screen.findByRole("heading", {
      name: "usage.display.title",
    });
    const pricing = screen.getByRole("heading", { name: "usage.tabs.pricing" });
    expect(
      display.compareDocumentPosition(pricing) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(screen.getByRole("textbox")).toHaveValue("6.8");

    // 定价表是单独的二级页，打开时抽屉先关
    expect(
      screen.queryByTestId("pricing-config-panel"),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "usage.settings.managePricing" }),
    );
    expect(
      await screen.findByTestId("pricing-config-panel"),
    ).toBeInTheDocument();
    // 不是模态框：新增 / 编辑定价的二级页叠在上面也要能点（模态框会把 body 设成不接指针事件）
    await waitFor(() =>
      expect(document.body.style.pointerEvents).not.toBe("none"),
    );
  });

  it("wires the tabs to the tabpanel and supports arrow / Home / End keys", async () => {
    const user = userEvent.setup();
    renderDashboard();

    const logsTab = screen.getByRole("tab", { name: /^usage\.requestLogs/ });
    expect(logsTab).toHaveAttribute("id", "usage-tab-logs");
    expect(logsTab).toHaveAttribute("aria-controls", "usage-tabpanel");
    expect(screen.getByRole("tabpanel")).toHaveAttribute(
      "aria-labelledby",
      "usage-tab-logs",
    );

    logsTab.focus();
    await user.keyboard("{ArrowRight}");
    const providersTab = screen.getByRole("tab", {
      name: /^usage\.tabs\.providers/,
    });
    expect(providersTab).toHaveAttribute("aria-selected", "true");
    expect(providersTab).toHaveFocus();
    expect(screen.getByRole("tabpanel")).toHaveAttribute(
      "aria-labelledby",
      "usage-tab-providers",
    );

    await user.keyboard("{End}");
    expect(
      screen.getByRole("tab", { name: /^usage\.tabs\.models/ }),
    ).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(logsTab).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowLeft}");
    expect(
      screen.getByRole("tab", { name: /^usage\.tabs\.models/ }),
    ).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Home}");
    expect(logsTab).toHaveFocus();
  });

  it("links to routing settings from the settings drawer", async () => {
    const user = userEvent.setup();
    const onOpenRoutingSettings = vi.fn();
    renderDashboard({ onOpenRoutingSettings });

    await openSettings(user);
    await user.click(
      screen.getByRole("button", { name: /usage.sources.editLogging/ }),
    );
    expect(onOpenRoutingSettings).toHaveBeenCalledTimes(1);
  });

  it("shows the empty state when there is no usage at all", async () => {
    usageApiMock.getUsageSummary.mockResolvedValue({ totalRequests: 0 });
    renderDashboard();

    expect(await screen.findByText("usage.empty.title")).toBeInTheDocument();
    expect(screen.queryByTestId("usage-hero")).not.toBeInTheDocument();
  });

  it("still lets pricing be opened from the empty state", async () => {
    usageApiMock.getUsageSummary.mockResolvedValue({ totalRequests: 0 });
    const user = userEvent.setup();
    renderDashboard();

    await screen.findByText("usage.empty.title");
    expect(
      screen.queryByTestId("pricing-config-panel"),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "usage.empty.configurePricing" }),
    );
    expect(
      await screen.findByTestId("pricing-config-panel"),
    ).toBeInTheDocument();
    // 空库里没有可看的概览，不画全是 0 的卡
    expect(screen.queryByTestId("usage-hero")).not.toBeInTheDocument();
  });
});

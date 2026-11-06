import { screen } from "@testing-library/react";
import { renderWithQueryClient as render } from "../utils/testQueryClient";
import { describe, expect, it, vi } from "vitest";
import { ProviderStatsTable } from "@/components/usage/ProviderStatsTable";
import { getStatsSpeed } from "@/components/usage/statsColumns";
import type { ProviderStats } from "@/types/usage";

const useProviderStatsMock = vi.hoisted(() => vi.fn());

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
    i18n: { resolvedLanguage: "en", language: "en" },
  }),
}));

vi.mock("@/lib/query/usage", () => ({
  useProviderStats: (...args: unknown[]) => useProviderStatsMock(...args),
}));

const stat = (overrides: Partial<ProviderStats>): ProviderStats => ({
  providerId: "p",
  appType: "claude",
  providerName: "P",
  requestCount: 1,
  totalTokens: 1_000,
  totalCost: "1",
  successRate: 100,
  avgLatencyMs: 1_000,
  ...overrides,
});

// 列顺序：…成功率、速度、首字
const speedCell = (row: HTMLElement) => row.children[row.children.length - 2];
const firstTokenCell = (row: HTMLElement) => row.lastElementChild;

describe("ProviderStatsTable", () => {
  it("computes speed as total output over total generation time", () => {
    expect(
      getStatsSpeed(
        stat({ speedOutputTokens: 1_200, speedGenerationMs: 10_500 }),
      ),
    ).toBe("114");
    expect(
      getStatsSpeed(stat({ speedOutputTokens: 0, speedGenerationMs: 0 })),
    ).toBeNull();
    // 老后端没有这两个字段
    expect(getStatsSpeed(stat({}))).toBeNull();
  });

  it("sorts by requests and replaces average latency with speed", () => {
    useProviderStatsMock.mockReturnValue({
      isLoading: false,
      data: [
        stat({
          providerId: "a",
          providerName: "Kimi For Coding",
          requestCount: 3,
        }),
        stat({
          providerId: "b",
          providerName: "DeepSeek",
          requestCount: 9,
          speedOutputTokens: 9_200,
          speedGenerationMs: 100_000,
        }),
      ],
    });

    render(
      <ProviderStatsTable range={{ preset: "7d" }} refreshIntervalMs={0} />,
    );

    expect(screen.queryByText("usage.avgLatency")).not.toBeInTheDocument();
    expect(
      screen.getByRole("columnheader", { name: /usage.speed/ }),
    ).toBeInTheDocument();
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("DeepSeek");
    expect(speedCell(rows[0])).toHaveTextContent("92tok/s");
    expect(rows[1]).toHaveTextContent("Kimi For Coding");
    expect(speedCell(rows[1])).toHaveTextContent("—");
  });

  it("shows the first-token median in seconds, or — without routed streams", () => {
    useProviderStatsMock.mockReturnValue({
      isLoading: false,
      data: [
        stat({ providerId: "a", requestCount: 5, firstTokenMedianMs: 2_640 }),
        stat({ providerId: "b", requestCount: 3, firstTokenMedianMs: null }),
        // 老后端没有这个字段
        stat({ providerId: "c", requestCount: 1 }),
      ],
    });

    render(
      <ProviderStatsTable range={{ preset: "7d" }} refreshIntervalMs={0} />,
    );

    expect(
      screen.getByRole("columnheader", { name: /usage.firstToken/ }),
    ).toBeInTheDocument();
    const rows = screen.getAllByRole("row").slice(1);
    expect(firstTokenCell(rows[0])).toHaveTextContent("2.6s");
    expect(firstTokenCell(rows[1])).toHaveTextContent("—");
    expect(firstTokenCell(rows[2])).toHaveTextContent("—");
  });

});

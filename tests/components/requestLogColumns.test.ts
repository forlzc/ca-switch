import { describe, expect, it } from "vitest";
import {
  REQUEST_LOG_COLUMNS,
  requestLogGroupBreaks,
  requestLogPinnedStart,
  resolveRequestLogColumns,
  setRequestLogColumn,
  TOGGLEABLE_REQUEST_LOG_COLUMNS,
} from "@/components/usage/requestLogColumns";

describe("request log columns", () => {
  it("shows the original columns by default and hides the new ones", () => {
    expect(resolveRequestLogColumns(undefined)).toEqual([
      "time",
      "provider",
      "model",
      "cost",
      "freshInput",
      "output",
      "cacheRead",
      "speed",
      "firstToken",
    ]);
  });

  it("applies overrides in table order, keeps time, ignores unknown ids", () => {
    expect(
      resolveRequestLogColumns({
        statusCode: true,
        time: false,
        someFutureColumn: true,
      }),
    ).toEqual([
      "time",
      "provider",
      "model",
      "cost",
      "freshInput",
      "output",
      "cacheRead",
      "speed",
      "firstToken",
      "statusCode",
    ]);
    expect(TOGGLEABLE_REQUEST_LOG_COLUMNS.map((c) => c.id)).not.toContain(
      "time",
    );
    expect(TOGGLEABLE_REQUEST_LOG_COLUMNS).toHaveLength(
      REQUEST_LOG_COLUMNS.length - 1,
    );
  });

  it("breaks groups at the first shown column of each group", () => {
    expect([
      ...requestLogGroupBreaks([
        "time",
        "model",
        "cost",
        "output",
        "cacheRead",
        "firstToken",
      ]),
    ]).toEqual(["cost", "output", "firstToken"]);
    expect(requestLogGroupBreaks(["time", "provider"]).size).toBe(0);
  });

  it("pins only the time column when it leads the table", () => {
    expect(requestLogPinnedStart(["time", "provider", "model"])).toEqual([
      "time",
    ]);
    expect(requestLogPinnedStart(["provider", "time"])).toEqual([]);
  });

  it("stores only columns that differ from their default", () => {
    expect(setRequestLogColumn(undefined, "hitRate", true)).toEqual({
      hitRate: true,
    });
    expect(setRequestLogColumn({ hitRate: true }, "hitRate", false)).toEqual(
      {},
    );
    expect(setRequestLogColumn({}, "provider", false)).toEqual({
      provider: false,
    });
    // 时间列不能关；别的版本写的未知列原样留着
    expect(
      setRequestLogColumn({ someFutureColumn: true }, "time", false),
    ).toEqual({ someFutureColumn: true });
  });
});

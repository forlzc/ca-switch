import { invoke } from "@tauri-apps/api/core";
import type { UsageResult } from "@/types";
import type { QuotaTier } from "@/types/subscription";

/** Coding Plan 额度查询结果 */
export interface CodingPlanQuotaResult {
  success: boolean;
  tiers: QuotaTier[];
  error?: string;
}

/** 通用用量模板（余额 / Coding Plan）走的后端命令，与具体客户端无关 */
export const subscriptionApi = {
  /** 余额查询（OpenAI 兼容 /v1/balance 等） */
  async getBalance(baseUrl: string, apiKey: string): Promise<UsageResult> {
    return await invoke<UsageResult>("get_balance", { baseUrl, apiKey });
  },

  /** Coding Plan 额度查询 */
  async getCodingPlanQuota(
    baseUrl: string,
    apiKey: string,
    accessKeyId?: string,
    secretAccessKey?: string,
    codingPlanProvider?: string,
    teamOrganizationId?: string,
    teamProjectId?: string,
  ): Promise<CodingPlanQuotaResult> {
    return await invoke<CodingPlanQuotaResult>("get_coding_plan_quota", {
      baseUrl,
      apiKey,
      accessKeyId,
      secretAccessKey,
      codingPlanProvider,
      teamOrganizationId,
      teamProjectId,
    });
  },
};

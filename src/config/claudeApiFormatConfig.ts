import type { ClaudeApiFormat } from "@/types";

export interface ClaudeApiFormatKeys {
  /** 写入时使用的首选变量名；读取时按 baseUrlKeys 依次兜底。 */
  baseUrlKeys: string[];
  apiKeyKeys: string[];
  modelKeys: string[];
}

/**
 * 上游协议与配置 JSON 里 env 变量名的对应关系。
 *
 * 配置 JSON 不再是 Anthropic 专用：选择哪种上游协议，表单/Key/地址/模型
 * 就使用该协议惯用的变量名；旧版 Anthropic 键作为兜底读取，保证存量行可用。
 */
export const CLAUDE_API_FORMAT_KEYS: Record<ClaudeApiFormat, ClaudeApiFormatKeys> = {
  anthropic: {
    baseUrlKeys: ["ANTHROPIC_BASE_URL"],
    apiKeyKeys: ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"],
    modelKeys: ["ANTHROPIC_MODEL"],
  },
  openai_chat: {
    baseUrlKeys: ["OPENAI_BASE_URL", "ANTHROPIC_BASE_URL"],
    apiKeyKeys: ["OPENAI_API_KEY"],
    modelKeys: ["OPENAI_MODEL", "ANTHROPIC_MODEL"],
  },
  openai_responses: {
    baseUrlKeys: ["OPENAI_BASE_URL", "ANTHROPIC_BASE_URL"],
    apiKeyKeys: ["OPENAI_API_KEY"],
    modelKeys: ["OPENAI_MODEL", "ANTHROPIC_MODEL"],
  },
  gemini_native: {
    baseUrlKeys: ["GOOGLE_GEMINI_BASE_URL"],
    apiKeyKeys: ["GEMINI_API_KEY"],
    modelKeys: ["GEMINI_MODEL", "ANTHROPIC_MODEL"],
  },
};

export function claudeConfigKeys(apiFormat: ClaudeApiFormat): ClaudeApiFormatKeys {
  return CLAUDE_API_FORMAT_KEYS[apiFormat] ?? CLAUDE_API_FORMAT_KEYS.anthropic;
}

/** 所有协议可能用到的 env 变量，切换协议/清理重复字段时使用。 */
export const ALL_CLAUDE_BASE_URL_KEYS = [
  "ANTHROPIC_BASE_URL",
  "OPENAI_BASE_URL",
  "GOOGLE_GEMINI_BASE_URL",
];
export const ALL_CLAUDE_API_KEY_KEYS = [
  "ANTHROPIC_AUTH_TOKEN",
  "ANTHROPIC_API_KEY",
  "OPENAI_API_KEY",
  "GEMINI_API_KEY",
];
export const ALL_CLAUDE_MODEL_KEYS = [
  "ANTHROPIC_MODEL",
  "OPENAI_MODEL",
  "GEMINI_MODEL",
];

/** 从 env 中按候选键顺序读取首个非空字符串。 */
export function readEnvValue(
  config: unknown,
  keys: string[],
): string {
  const env = (config as { env?: Record<string, unknown> } | undefined)?.env;
  if (!env || typeof env !== "object") return "";
  for (const key of keys) {
    const value = env[key];
    if (typeof value === "string" && value.trim()) return value;
  }
  return "";
}

/**
 * 把值写入首选键，并清掉 removeKeys 里的其它键，避免同一字段出现多份。
 * 值为空时删除全部候选键。
 */
export function writeEnvValue(
  config: Record<string, unknown>,
  keys: string[],
  value: string,
  removeKeys: string[] = keys,
): void {
  const env =
    typeof config.env === "object" && config.env !== null
      ? (config.env as Record<string, unknown>)
      : {};
  const trimmed = value.trim();
  if (trimmed) {
    env[keys[0]] = trimmed;
  }
  for (const key of removeKeys) {
    if (key !== keys[0]) delete env[key];
  }
  if (!trimmed && keys.length > 0) {
    delete env[keys[0]];
  }
  config.env = env;
}

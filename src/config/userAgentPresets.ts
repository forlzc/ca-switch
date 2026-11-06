/**
 * 自定义 User-Agent 预设。
 *
 * 取值来自 PR #3671 对 Kimi Coding Plan（api.kimi.com/coding）UA 白名单的 curl 实测：
 * `claude-cli/*`、`claude-code/*`、`Kilo-Code/*` 可通过；`codex-cli`、`kimi-cli` 会被 403。
 * 白名单只校验 UA 名称前缀、不看版本号，因此用静态值即可，版本不会因客户端升级而失效。
 * 2026-09-09 更正：Kimi Code 已官方支持 Codex 直连，真机 codex_cli_rs/0.153.4 打
 * api.kimi.com/coding/v1/responses 全程放行——上面的 403 结论只对当年的 Chat/Anthropic
 * 路径成立，Codex 预设已改原生 Responses 直连，不再需要借这些预设伪装。
 *
 * 列表按客户端分组展示（Claude Code / Codex / 其他）。同一组内顺序即推荐优先级：
 * 越靠前越贴近该客户端真机发出的完整格式，最稳过严格的 UA 校验；靠后的为简短变体或
 * 旧前缀，供上游只认某个历史前缀时兜底。
 *
 * 这些预设用于「Coding Agent 想接入受 UA 限制的上游」的场景——把转发请求伪装成
 * 已在白名单内的客户端。是否使用由用户显式选择。
 */

export type UserAgentPresetClient = "claude-code" | "codex" | "other";

export interface UserAgentPreset {
  /** 分组：决定下拉菜单里的分组标题与组内顺序。 */
  client: UserAgentPresetClient;
  /** 实际写进请求头的 User-Agent 值。 */
  value: string;
}

export const USER_AGENT_PRESETS: readonly UserAgentPreset[] = [
  // ── Claude Code ──────────────────────────────────────────────
  // 当前 Claude Code CLI 实际发送的完整格式：名称/版本 + (external, cli) 后缀。
  // 版本按 npm @anthropic-ai/claude-code latest 核对（2026-11-06：2.1.318）。
  { client: "claude-code", value: "claude-cli/2.1.318 (external, cli)" },
  // 不带后缀的简短变体。
  { client: "claude-code", value: "claude-cli/2.1.318" },
  // 上一基线，保留给按旧版本做灰度/白名单的上游。
  { client: "claude-code", value: "claude-cli/2.1.161 (external, cli)" },
  // 后端做 Codex→Claude 模拟时使用的基线值（proxy/forwarder.rs 的 CLAUDE_CODE_USER_AGENT）。
  { client: "claude-code", value: "claude-cli/1.0.119 (external, cli)" },
  // 早期检测口径使用的版本，上游按版本做灰度/白名单时更稳。
  { client: "claude-code", value: "claude-cli/2.1.2 (external, cli)" },
  // 旧 SDK 前缀，部分中转按 claude-code/* 放行。
  { client: "claude-code", value: "claude-code/1.0.0" },
  { client: "claude-code", value: "claude-code/0.1.0" },

  // ── Codex ───────────────────────────────────────────────────
  // 版本按 npm @openai/codex latest 核对（2026-11-06：0.167.0）。
  { client: "codex", value: "codex_cli_rs/0.167.0" },
  // 带平台串的完整格式：name/version (OS; arch) Terminal。
  {
    client: "codex",
    value: "codex_cli_rs/0.167.0 (Windows 10.0.26100; x86_64) WindowsTerminal",
  },
  // 真机实测版本（Kimi Coding Plan 放行记录）。
  { client: "codex", value: "codex_cli_rs/0.153.4" },
  {
    client: "codex",
    value: "codex_cli_rs/0.77.0 (Windows 10.0.26100; x86_64) WindowsTerminal",
  },
  { client: "codex", value: "codex_cli_rs/0.153.4 (Windows 10.0.26100; x86_64) WindowsTerminal" },

  // ── 其他 ────────────────────────────────────────────────────
  // 版本按 npm @kilocode/cli / Open VSX latest 核对（2026-11-06：7.8.27）。
  { client: "other", value: "Kilo-Code/7.8.27" },
  // 实测可通过 ps.air-outer.com（该网关按此版本放行）。
  { client: "other", value: "Kilo-Code/7.3.50" },
  { client: "other", value: "Kilo-Code/1.0" },
];

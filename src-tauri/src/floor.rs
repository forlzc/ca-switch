//! 各客户端配置里属于「某一家」的键。
//!
//! 账号目录之间的共享层同步要跳过这些键，`--settings` 临时文件也要把它们挡住：它们记的是
//! 请求发到哪、凭什么鉴权、哪个模型名，换一家就不成立了。其余键归用户和客户端，可以跟着走。
//!
//! 只有「整个前缀都属于连接和鉴权」的地方才按前缀匹配（`ANTHROPIC_*`、`AWS_*`）。漏掉的
//! 键会被当作用户键原样保留，失败方向是安全的。

/// Claude Code 的协议选择器。
///
/// `CLAUDE_CODE_USE_` 不能按前缀匹配：同一前缀下还有 `USE_POWERSHELL_TOOL`、
/// `USE_NATIVE_FILE_SEARCH`、`USE_COWORK_PLUGINS`、`USE_CCR_V2` 这类与供应商无关的
/// 功能开关。
pub const CLAUDE_PROTOCOL_SELECTORS: &[&str] = &[
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_GATEWAY",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_USE_ANTHROPIC_AWS",
    "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
];

/// `env` 里整个前缀都属于连接和鉴权的前缀。
pub const CLAUDE_FLOOR_ENV_PREFIXES: &[&str] = &["ANTHROPIC_", "AWS_", "VERTEX_REGION_"];

/// `env` 里按名字列出的关键字段（协议选择器之外）。
pub const CLAUDE_FLOOR_ENV_KEYS: &[&str] = &[
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
    "CLOUD_ML_REGION",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "CLAUDE_CODE_OAUTH_SCOPES",
    "CLAUDE_CODE_API_KEY_HELPER_TTL_MS",
];

/// Claude Code `settings.json` 顶层的关键字段。
pub const CLAUDE_FLOOR_TOP: &[&str] = &[
    "apiKeyHelper",
    "apiBaseUrl",
    "primaryModel",
    "smallFastModel",
    "apiKey",
    "model",
    "fallbackModel",
    "modelOverrides",
    "modelPicker",
    "advisorModel",
    "awsAuthRefresh",
    "awsCredentialExport",
    "gcpAuthRefresh",
];

/// Claude Code 的供应商独有字段（`env` 里）：上游的兼容开关和窗口值。
pub const CLAUDE_EXCLUSIVE_ENV: &[&str] = &[
    "CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",
    "CLAUDE_CODE_DISABLE_ARTIFACT",
    "ENABLE_TOOL_SEARCH",
    "CLAUDE_CODE_DISABLE_THINKING",
    "DISABLE_INTERLEAVED_THINKING",
    "CLAUDE_CODE_ALWAYS_ENABLE_EFFORT",
    "CLAUDE_CODE_EXTRA_BODY",
    "CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING",
    "CLAUDE_CODE_AUTO_MODE_SERVER",
    "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
    "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
    "CLAUDE_CODE_DISABLE_1M_CONTEXT",
    "CLAUDE_CODE_DISABLE_UNKNOWN_MODEL_WINDOW_ENFORCEMENT",
    "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
];

/// Claude Code `settings.json` 的 `env` 里，这个键是否是关键字段。
pub fn claude_floor_env(key: &str) -> bool {
    CLAUDE_FLOOR_ENV_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
        || CLAUDE_PROTOCOL_SELECTORS.contains(&key)
        || CLAUDE_FLOOR_ENV_KEYS.contains(&key)
        || (key.starts_with("CLAUDE_CODE_SKIP_") && key.ends_with("_AUTH"))
}

/// Claude Code `settings.json` 顶层的关键字段。
pub fn claude_floor_top(key: &str) -> bool {
    CLAUDE_FLOOR_TOP.contains(&key)
}

/// Claude Code 的供应商独有字段（`env` 里）。
pub fn claude_exclusive_env(key: &str) -> bool {
    CLAUDE_EXCLUSIVE_ENV.contains(&key)
}

/// Codex `config.toml` 顶层的关键字段。另有 `[model_providers.custom]` 整表。
pub const CODEX_FLOOR_TOP: &[&str] = &[
    "model_provider",
    "openai_base_url",
    "model",
    "review_model",
    "model_reasoning_effort",
    "plan_mode_reasoning_effort",
    "disable_response_storage",
    "model_catalog_json",
    "experimental_bearer_token",
    "base_url",
    "wire_api",
];

/// 嵌套在用户自己的表里的模型名：只清这几个键，表里的其他键不动。
pub const CODEX_FLOOR_NESTED: &[&[&str]] = &[
    &["agents", "default_subagent_model"],
    &["agents", "default_subagent_reasoning_effort"],
    &["memories", "extract_model"],
    &["memories", "consolidation_model"],
];

/// Codex 的供应商独有字段（顶层）。
pub const CODEX_EXCLUSIVE_TOP: &[&str] = &[
    "web_search",
    "model_context_window",
    "model_auto_compact_token_limit",
    "model_supports_reasoning_summaries",
    "model_verbosity",
];

/// Codex 自定义供应商表的 id。
pub const CODEX_ROUTE_ID: &str = "custom";

/// 顶层的关键字段里，直接取行里值的那几个。
pub const CODEX_ROW_TOP_FIELDS: &[&str] = &[
    "model",
    "review_model",
    "model_reasoning_effort",
    "plan_mode_reasoning_effort",
    "disable_response_storage",
];

/// Gemini CLI `.env` 里，这个键是否是关键字段。
///
/// `GOOGLE_*` 整个前缀都是连接和鉴权（Gemini CLI 0.61.0 读取的全部如此）。
/// `GEMINI_*` 不能按前缀：同一前缀下有 `GEMINI_CLI_HOME`、`GEMINI_SANDBOX`、
/// `GEMINI_SYSTEM_MD`、遥测开关等与供应商无关的设置。
pub fn gemini_floor_env(key: &str) -> bool {
    key.starts_with("GOOGLE_")
        || matches!(
            key,
            "GEMINI_API_KEY"
                | "GEMINI_MODEL"
                | "GEMINI_API_KEY_AUTH_MECHANISM"
                | "GEMINI_CLI_CUSTOM_HEADERS"
                | "GEMINI_DEFAULT_AUTH_TYPE"
                | "GEMINI_CLI_USE_COMPUTE_ADC"
                | "CODE_ASSIST_ENDPOINT"
                | "CODE_ASSIST_API_VERSION"
        )
}

/// Claude Desktop 的 `configLibrary/<id>.json` 里的关键字段。
pub const DESKTOP_PROFILE_FLOOR: &[&str] = &[
    "inferenceProvider",
    "inferenceGatewayBaseUrl",
    "inferenceGatewayApiKey",
    "inferenceGatewayAuthScheme",
    "inferenceModels",
    "modelDiscoveryEnabled",
];

/// Claude Desktop profile 里的策略键：缺失时写入，存在时不动。
pub const DESKTOP_PROFILE_SEED: &[&str] = &["disableDeploymentModeChooser", "coworkEgressAllowedHosts"];

pub fn desktop_profile_floor(key: &str) -> bool {
    DESKTOP_PROFILE_FLOOR.contains(&key)
}

/// Codex 写进 live 的供应商表。
pub const CODEX_PROVIDER_TABLE: &[&str] = &["model_providers", "custom"];

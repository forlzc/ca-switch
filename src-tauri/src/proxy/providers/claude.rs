//! Claude (Anthropic) Provider Adapter
//!
//! 支持透传模式和 OpenAI 格式转换模式
//!
//! ## API 格式
//! - **anthropic** (默认): Anthropic Messages API 格式，直接透传
//! - **openai_chat**: OpenAI Chat Completions 格式，需要 Anthropic ↔ OpenAI 转换
//! - **openai_responses**: OpenAI Responses API 格式，需要 Anthropic ↔ Responses 转换
//! - **gemini_native**: Google Gemini Native generateContent 格式，需要 Anthropic ↔ Gemini 转换
//!
//! ## 认证模式
//! - **Claude**: Anthropic 官方 API (x-api-key + anthropic-version)
//! - **ClaudeAuth**: 中转服务 (仅 Bearer 认证，无 x-api-key)
//! - **OpenRouter**: 已支持 Claude Code 兼容接口，默认透传
//! - **GitHubCopilot**: GitHub Copilot (OAuth + Copilot Token)

use super::{AuthInfo, AuthStrategy, ProviderAdapter, ProviderType};
use crate::provider::Provider;
use crate::proxy::error::ProxyError;
use serde_json::{json, Value};

const ANTHROPIC_THINKING_PLACEHOLDER: &str = "tool call";
const ANTHROPIC_REDACTED_THINKING_PLACEHOLDER: &str = "[redacted thinking]";
// Keep hints lowercase; matching lowercases only the input value.
// Moonshot/Kimi exited on vendor request (2026-08): their endpoints no longer
// require thinking replay on tool-call turns, and injected placeholders
// disrupt the model's chain of thought. Do not re-add without re-confirming.
const REASONING_VENDOR_HINTS: &[&str] = &["deepseek", "mimo", "xiaomimimo"];

/// 获取 Claude 供应商的 API 格式
///
/// 供 handler/forwarder 外部使用的公开函数。
/// 优先级：meta.apiFormat > settings_config.api_format > openrouter_compat_mode > 默认 "anthropic"
pub fn get_claude_api_format(provider: &Provider) -> &'static str {
    // 1) Preferred: meta.apiFormat (SSOT, never written to Claude Code config)
    if let Some(meta) = provider.meta.as_ref() {
        if let Some(api_format) = meta.api_format.as_deref() {
            return match api_format {
                "openai_chat" => "openai_chat",
                "openai_responses" => "openai_responses",
                "gemini_native" => "gemini_native",
                _ => "anthropic",
            };
        }
    }

    // 2) Backward compatibility: legacy settings_config.api_format
    if let Some(api_format) = provider
        .settings_config
        .get("api_format")
        .and_then(|v| v.as_str())
    {
        return match api_format {
            "openai_chat" => "openai_chat",
            "openai_responses" => "openai_responses",
            "gemini_native" => "gemini_native",
            _ => "anthropic",
        };
    }

    // 3) Backward compatibility: legacy openrouter_compat_mode (bool/number/string)
    let raw = provider.settings_config.get("openrouter_compat_mode");
    let enabled = match raw {
        Some(serde_json::Value::Bool(v)) => *v,
        Some(serde_json::Value::Number(num)) => num.as_i64().unwrap_or(0) != 0,
        Some(serde_json::Value::String(value)) => {
            let normalized = value.trim().to_lowercase();
            normalized == "true" || normalized == "1"
        }
        _ => false,
    };

    if enabled {
        "openai_chat"
    } else {
        "anthropic"
    }
}

pub fn claude_api_format_needs_transform(api_format: &str) -> bool {
    matches!(
        api_format,
        "openai_chat" | "openai_responses" | "gemini_native"
    )
}

fn is_reasoning_vendor_identifier(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    REASONING_VENDOR_HINTS
        .iter()
        .any(|hint| value.contains(hint))
}

fn should_normalize_anthropic_tool_thinking_history(
    provider: &Provider,
    body: &Value,
    api_format: &str,
) -> bool {
    if api_format.trim() != "anthropic" {
        return false;
    }

    if body
        .get("model")
        .and_then(|m| m.as_str())
        .is_some_and(is_reasoning_vendor_identifier)
    {
        return true;
    }

    let settings = &provider.settings_config;
    [
        settings
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|v| v.as_str()),
        settings.get("base_url").and_then(|v| v.as_str()),
        settings.get("baseURL").and_then(|v| v.as_str()),
        settings.get("apiEndpoint").and_then(|v| v.as_str()),
    ]
    .into_iter()
    .flatten()
    .any(is_reasoning_vendor_identifier)
}

/// DeepSeek's Anthropic-compatible endpoint requires thinking history to be
/// replayed on every assistant turn that contains tool_use. Some Anthropic SDK
/// clients keep the tool history but drop or redact the thinking block, which
/// makes DeepSeek reject the next request with `content[].thinking ... must be
/// passed back`. Normalize only the narrow tool-call history shape for
/// providers known to require plain `thinking` blocks.
pub fn normalize_anthropic_tool_thinking_history_for_provider(
    body: &mut Value,
    provider: &Provider,
    api_format: &str,
) -> bool {
    if !should_normalize_anthropic_tool_thinking_history(provider, body, api_format) {
        return false;
    }

    normalize_anthropic_tool_thinking_history(body)
}

/// DeepSeek official Anthropic-compatible endpoint URL
const DEEPSEEK_OFFICIAL_ANTHROPIC_URL: &str = "https://api.deepseek.com/anthropic";

/// Check whether the provider is configured to use DeepSeek's official
/// Anthropic-compatible endpoint.
fn is_deepseek_official_anthropic_endpoint(provider: &Provider) -> bool {
    let settings = &provider.settings_config;
    let base_url = settings
        .get("env")
        .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
        .and_then(|v| v.as_str())
        .or_else(|| settings.get("base_url").and_then(|v| v.as_str()))
        .or_else(|| settings.get("baseURL").and_then(|v| v.as_str()))
        .or_else(|| settings.get("apiEndpoint").and_then(|v| v.as_str()));

    base_url.map(|u| u.trim_end_matches('/')) == Some(DEEPSEEK_OFFICIAL_ANTHROPIC_URL)
}

/// DeepSeek's official Anthropic-compatible endpoint treats
/// `thinking: { type: "disabled" }` and effort parameters (`output_config.effort`
/// or `reasoning_effort`) as mutually exclusive, returning HTTP 400:
/// "thinking options type cannot be disabled when reasoning_effort is set".
/// This breaks Claude Code 2.1.166+ Workflow/Dynamic Workflow features.
///
/// Rather than overriding Claude Code's intentional `thinking: disabled` for
/// sub-agents, we respect that decision and remove the conflicting effort
/// parameters instead. `thinking: disabled` means "don't output thinking
/// blocks", which is the correct behavior for sub-agents that don't need
/// to display reasoning to the user.
///
/// <https://github.com/deepseek-ai/DeepSeek-V3/issues/1397>
pub fn normalize_deepseek_thinking_disabled_strip_effort(
    body: &mut Value,
    provider: &Provider,
) -> bool {
    if !is_deepseek_official_anthropic_endpoint(provider) {
        return false;
    }

    let thinking_type = body
        .get("thinking")
        .and_then(|t| t.get("type"))
        .and_then(|t| t.as_str());

    if thinking_type != Some("disabled") {
        return false;
    }

    let mut changed = false;

    // Remove output_config.effort (Anthropic format)
    if let Some(oc) = body
        .get_mut("output_config")
        .and_then(|v| v.as_object_mut())
    {
        changed |= oc.remove("effort").is_some();
        // Clean up empty output_config
        if oc.is_empty() {
            body.as_object_mut().unwrap().remove("output_config");
        }
    }

    // Remove reasoning_effort (OpenAI format, may be present in passthrough)
    if body.get("reasoning_effort").is_some() {
        body.as_object_mut().unwrap().remove("reasoning_effort");
        changed = true;
    }

    changed
}

pub fn normalize_anthropic_messages_for_provider(
    body: &mut Value,
    provider: &Provider,
    api_format: &str,
) -> bool {
    if api_format.trim() != "anthropic" {
        return false;
    }

    let mut changed =
        normalize_anthropic_tool_thinking_history_for_provider(body, provider, api_format);
    changed |= normalize_deepseek_thinking_disabled_strip_effort(body, provider);
    changed
}

fn normalize_anthropic_tool_thinking_history(body: &mut Value) -> bool {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return false;
    };

    let mut changed = false;
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }

        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        if !content
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        {
            continue;
        }

        let mut has_thinking = false;
        for block in content.iter_mut() {
            match block.get("type").and_then(Value::as_str) {
                Some("thinking") => {
                    let has_non_empty_thinking = block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.trim().is_empty());
                    if let Some(obj) = block.as_object_mut() {
                        if obj.remove("signature").is_some() {
                            changed = true;
                        }
                        if !has_non_empty_thinking {
                            obj.insert(
                                "thinking".to_string(),
                                json!(ANTHROPIC_THINKING_PLACEHOLDER),
                            );
                            changed = true;
                        }
                    }
                    has_thinking = true;
                }
                Some("redacted_thinking") => {
                    *block = json!({
                        "type": "thinking",
                        "thinking": ANTHROPIC_REDACTED_THINKING_PLACEHOLDER
                    });
                    has_thinking = true;
                    changed = true;
                }
                _ => {}
            }
        }

        if !has_thinking {
            content.insert(
                0,
                json!({
                    "type": "thinking",
                    "thinking": ANTHROPIC_THINKING_PLACEHOLDER
                }),
            );
            changed = true;
        }
    }

    changed
}

fn should_preserve_reasoning_content_for_openai_chat(provider: &Provider, body: &Value) -> bool {
    if body
        .get("model")
        .and_then(|m| m.as_str())
        .is_some_and(is_reasoning_vendor_identifier)
    {
        return true;
    }

    let settings = &provider.settings_config;
    let base_urls = [
        settings
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|v| v.as_str()),
        settings.get("base_url").and_then(|v| v.as_str()),
        settings.get("baseURL").and_then(|v| v.as_str()),
        settings.get("apiEndpoint").and_then(|v| v.as_str()),
    ];

    base_urls
        .into_iter()
        .flatten()
        .any(is_reasoning_vendor_identifier)
}

pub fn transform_claude_request_for_api_format(
    body: serde_json::Value,
    provider: &Provider,
    api_format: &str,
    session_id: Option<&str>,
    shadow_store: Option<&super::gemini_shadow::GeminiShadowStore>,
) -> Result<serde_json::Value, ProxyError> {
    let session_cache_key: Option<String> = session_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);

    let explicit_cache_key = provider
        .meta
        .as_ref()
        .and_then(|m| m.prompt_cache_key.as_deref());
    let (cache_key, cache_key_source) = if let Some(key) = explicit_cache_key {
        (Some(key), "explicit")
    } else if let Some(key) = session_cache_key.as_deref() {
        (Some(key), "session")
    } else {
        (None, "none")
    };
    // 转换层只给认得的模型写思考档位，其余模型由 claude_reasoning 补判（#9062）。
    let effort_plan = if matches!(api_format, "openai_chat" | "openai_responses") {
        let base_url = ClaudeAdapter::new()
            .extract_base_url(provider)
            .unwrap_or_default();
        super::claude_reasoning::plan(&body, provider, &base_url)
    } else {
        super::claude_reasoning::EffortPlan::Unchanged
    };
    match api_format {
        "openai_responses" => {
            log::debug!(
                "[Cache] OpenAI Responses prompt_cache_key source={cache_key_source}, provider={}, has_key={}",
                provider.id,
                cache_key.is_some()
            );
            let mut result = super::transform_responses::anthropic_to_responses(
                body, cache_key, false, false,
            )?;
            super::claude_reasoning::apply(&mut result, api_format, effort_plan);
            Ok(result)
        }
        "openai_chat" => {
            let preserve_reasoning_content =
                should_preserve_reasoning_content_for_openai_chat(provider, &body);
            let mut result = super::transform::anthropic_to_openai_with_reasoning_content(
                body,
                preserve_reasoning_content,
            )?;
            // Inject prompt_cache_key only if explicitly configured in meta
            if let Some(key) = provider
                .meta
                .as_ref()
                .and_then(|m| m.prompt_cache_key.as_deref())
            {
                result["prompt_cache_key"] = serde_json::json!(key);
            }
            // 流式请求必须注入 stream_options.include_usage，否则 OpenAI 兼容上游
            // 不在 SSE 末尾吐 usage → 转换出的 Anthropic message_delta 全 0 →
            // 整笔 input/output/cache 漏记（与 Codex Responses→Chat 路径同源）。
            super::transform::inject_openai_stream_include_usage(&mut result);
            super::claude_reasoning::apply(&mut result, api_format, effort_plan);
            Ok(result)
        }
        "gemini_native" => super::transform_gemini::anthropic_to_gemini_with_shadow(
            body,
            shadow_store,
            Some(&provider.id),
            session_id,
        ),
        _ => Ok(body),
    }
}

/// Claude 适配器
pub struct ClaudeAdapter;

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self
    }

    /// 获取供应商类型
    ///
    /// 根据 base_url 和 auth_mode 检测具体的供应商类型：
    /// - OpenRouter: base_url 包含 openrouter.ai
    /// - ClaudeAuth: auth_mode 为 bearer_only
    /// - Claude: 默认 Anthropic 官方
    pub fn provider_type(&self, provider: &Provider) -> ProviderType {
        // 检测 Gemini Native 格式
        if self.get_api_format(provider) == "gemini_native" {
            return match self.extract_key(provider) {
                Some(key) if key.starts_with("ya29.") || key.starts_with('{') => {
                    ProviderType::GeminiCli
                }
                _ => ProviderType::Gemini,
            };
        }

        // 检测 OpenRouter
        if self.is_openrouter(provider) {
            return ProviderType::OpenRouter;
        }

        // 检测 ClaudeAuth (仅 Bearer 认证)
        if self.is_bearer_only_mode(provider) {
            return ProviderType::ClaudeAuth;
        }

        ProviderType::Claude
    }

    /// 检测是否使用 OpenRouter
    fn is_openrouter(&self, provider: &Provider) -> bool {
        if let Ok(base_url) = self.extract_base_url(provider) {
            return base_url.contains("openrouter.ai");
        }
        false
    }

    /// 获取 API 格式
    ///
    /// 从 provider.meta.api_format 读取格式设置：
    /// - "anthropic" (默认): Anthropic Messages API 格式，直接透传
    /// - "openai_chat": OpenAI Chat Completions 格式，需要格式转换
    /// - "openai_responses": OpenAI Responses API 格式，需要格式转换
    fn get_api_format(&self, provider: &Provider) -> &'static str {
        get_claude_api_format(provider)
    }

    /// 检测是否为仅 Bearer 认证模式
    fn is_bearer_only_mode(&self, provider: &Provider) -> bool {
        // 检查 settings_config 中的 auth_mode
        if let Some(auth_mode) = provider
            .settings_config
            .get("auth_mode")
            .and_then(|v| v.as_str())
        {
            if auth_mode == "bearer_only" {
                return true;
            }
        }

        // 检查 env 中的 AUTH_MODE
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(auth_mode) = env.get("AUTH_MODE").and_then(|v| v.as_str()) {
                if auth_mode == "bearer_only" {
                    return true;
                }
            }
        }

        false
    }

    /// 从 Provider 配置中提取 API Key
    fn extract_key(&self, provider: &Provider) -> Option<String> {
        if let Some(env) = provider.settings_config.get("env") {
            // Anthropic 标准 key
            if let Some(key) = env
                .get("ANTHROPIC_AUTH_TOKEN")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                log::debug!("[Claude] 使用 ANTHROPIC_AUTH_TOKEN");
                return Some(key.to_string());
            }
            if let Some(key) = env
                .get("ANTHROPIC_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                log::debug!("[Claude] 使用 ANTHROPIC_API_KEY");
                return Some(key.to_string());
            }
            // OpenRouter key
            if let Some(key) = env
                .get("OPENROUTER_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                log::debug!("[Claude] 使用 OPENROUTER_API_KEY");
                return Some(key.to_string());
            }
            // 备选 OpenAI key (用于 OpenRouter)
            if let Some(key) = env
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                log::debug!("[Claude] 使用 OPENAI_API_KEY");
                return Some(key.to_string());
            }
            // Gemini Native key
            if let Some(key) = env
                .get("GEMINI_API_KEY")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                log::debug!("[Claude] 使用 GEMINI_API_KEY");
                return Some(key.to_string());
            }
            // Bedrock API Key：Claude Code 读的变量名。旧版预设写在顶层 apiKey，
            // 由下面的直接获取兜底。
            if let Some(key) = env
                .get("AWS_BEARER_TOKEN_BEDROCK")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                log::debug!("[Claude] 使用 AWS_BEARER_TOKEN_BEDROCK");
                return Some(key.to_string());
            }
        }

        // 尝试直接获取
        if let Some(key) = provider
            .settings_config
            .get("apiKey")
            .or_else(|| provider.settings_config.get("api_key"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            log::debug!("[Claude] 使用 apiKey/api_key");
            return Some(key.to_string());
        }

        log::warn!("[Claude] 未找到有效的 API Key");
        None
    }

    /// 根据 env 中填写的变量名推断 Anthropic 默认走哪种鉴权策略。
    ///
    /// 与 Anthropic SDK 原生语义保持一致：
    /// - `ANTHROPIC_AUTH_TOKEN` → `ClaudeAuth`（发送 `Authorization: Bearer`）
    /// - `ANTHROPIC_API_KEY`    → `Anthropic` （发送 `x-api-key`）
    ///
    /// 优先级与 [`extract_key`] 一致；两者都缺时返回 `None` 由调用方决定 fallback。
    fn infer_anthropic_auth_strategy(&self, provider: &Provider) -> Option<AuthStrategy> {
        let env = provider.settings_config.get("env")?;

        let has_value = |key: &str| -> bool {
            env.get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_some()
        };

        if has_value("ANTHROPIC_AUTH_TOKEN") {
            return Some(AuthStrategy::ClaudeAuth);
        }
        if has_value("ANTHROPIC_API_KEY") {
            return Some(AuthStrategy::Anthropic);
        }
        None
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderAdapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "Claude"
    }

    fn extract_base_url(&self, provider: &Provider) -> Result<String, ProxyError> {
        // 1. 从 env 中获取
        if let Some(env) = provider.settings_config.get("env") {
            if let Some(url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
                return Ok(url.trim_end_matches('/').to_string());
            }
        }

        // 2. 尝试直接获取
        if let Some(url) = provider
            .settings_config
            .get("base_url")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        if let Some(url) = provider
            .settings_config
            .get("baseURL")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        if let Some(url) = provider
            .settings_config
            .get("apiEndpoint")
            .and_then(|v| v.as_str())
        {
            return Ok(url.trim_end_matches('/').to_string());
        }

        Err(ProxyError::ConfigError(
            "Claude Provider 缺少 base_url 配置".to_string(),
        ))
    }

    fn extract_auth(&self, provider: &Provider) -> Option<AuthInfo> {
        let provider_type = self.provider_type(provider);

        let key = self.extract_key(provider)?;

        match provider_type {
            ProviderType::GeminiCli => {
                // Parse stored OAuth JSON and only attach access_token when
                // it's actually usable. `parse_oauth_credentials` accepts
                // refresh-token-only JSON (which is legitimate before the
                // first refresh) and also surfaces `{"access_token": "", ...}`
                // for expired credentials. In both cases we would otherwise
                // send `Authorization: Bearer ` to upstream and get a 401.
                //
                // CC Switch does not currently exchange the refresh_token for
                // a fresh access_token. Until that path exists, degrade to
                // plain GoogleOAuth strategy (which still sends the raw key
                // as a fallback) and log loudly so users know to refresh
                // their `~/.gemini/oauth_creds.json`.
                match super::gemini::GeminiAdapter::new().parse_oauth_credentials(&key) {
                    Some(creds) if !creds.access_token.is_empty() => {
                        Some(AuthInfo::with_access_token(key, creds.access_token))
                    }
                    Some(_) => {
                        log::warn!(
                            "[Gemini OAuth] access_token missing or empty for provider `{}`; \
                             bearer auth will likely fail with 401. Refresh \
                             ~/.gemini/oauth_creds.json via the gemini CLI to obtain a new token.",
                            provider.id
                        );
                        Some(AuthInfo::new(key, AuthStrategy::GoogleOAuth))
                    }
                    None => Some(AuthInfo::new(key, AuthStrategy::GoogleOAuth)),
                }
            }
            ProviderType::Gemini => Some(AuthInfo::new(key, AuthStrategy::Google)),
            ProviderType::OpenRouter => Some(AuthInfo::new(key, AuthStrategy::Bearer)),
            ProviderType::ClaudeAuth => Some(AuthInfo::new(key, AuthStrategy::ClaudeAuth)),
            _ => {
                // OpenAI Chat / Responses 协议只认 Authorization: Bearer；变量名推断只对
                // Anthropic Messages 上游有意义，选了 ANTHROPIC_API_KEY 也不能发 x-api-key。
                if matches!(
                    self.get_api_format(provider),
                    "openai_chat" | "openai_responses"
                ) {
                    return Some(AuthInfo::new(key, AuthStrategy::Bearer));
                }
                // 按 env 中的变量名推断鉴权策略，对齐 Anthropic SDK 语义：
                // ANTHROPIC_AUTH_TOKEN → Authorization: Bearer
                // ANTHROPIC_API_KEY    → x-api-key
                // 其他来源（apiKey 直填等）默认走 x-api-key（Anthropic 官方协议）。
                let strategy = self
                    .infer_anthropic_auth_strategy(provider)
                    .unwrap_or(AuthStrategy::Anthropic);
                Some(AuthInfo::new(key, strategy))
            }
        }
    }

    fn build_url(&self, base_url: &str, endpoint: &str) -> String {
        // NOTE:
        // 过去 OpenRouter 只有 OpenAI Chat Completions 兼容接口，需要把 Claude 的 `/v1/messages`
        // 映射到 `/v1/chat/completions`，并做 Anthropic ↔ OpenAI 的格式转换。
        //
        // 现在 OpenRouter 已推出 Claude Code 兼容接口，因此默认直接透传 endpoint。
        // 如需回退旧逻辑，可在 forwarder 中根据 needs_transform 改写 endpoint。
        //
        let mut base = format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            endpoint.trim_start_matches('/')
        );

        // 去除重复的 /v1/v1（可能由 base_url 与 endpoint 都带版本导致）
        while base.contains("/v1/v1") {
            base = base.replace("/v1/v1", "/v1");
        }

        base
    }

    fn get_auth_headers(
        &self,
        auth: &AuthInfo,
    ) -> Result<Vec<(http::HeaderName, http::HeaderValue)>, ProxyError> {
        use super::adapter::auth_header_value as hv;
        use http::{HeaderName, HeaderValue};
        // 注意：anthropic-version 由 forwarder.rs 统一处理（透传客户端值或设置默认值）
        let bearer = format!("Bearer {}", auth.api_key);
        Ok(match auth.strategy {
            AuthStrategy::Anthropic => {
                vec![(HeaderName::from_static("x-api-key"), hv(&auth.api_key)?)]
            }
            AuthStrategy::ClaudeAuth | AuthStrategy::Bearer => {
                vec![(HeaderName::from_static("authorization"), hv(&bearer)?)]
            }
            AuthStrategy::Google => vec![(
                HeaderName::from_static("x-goog-api-key"),
                hv(&auth.api_key)?,
            )],
            AuthStrategy::GoogleOAuth => {
                let token = auth.access_token.as_ref().unwrap_or(&auth.api_key);
                vec![
                    (
                        HeaderName::from_static("authorization"),
                        hv(&format!("Bearer {token}"))?,
                    ),
                    (
                        HeaderName::from_static("x-goog-api-client"),
                        HeaderValue::from_static("GeminiCLI/1.0"),
                    ),
                ]
            }
        })
    }

    fn needs_transform(&self, provider: &Provider) -> bool {
        // 根据 api_format 配置决定是否需要格式转换
        // - "anthropic" (默认): 直接透传，无需转换
        // - "openai_chat": 需要 Anthropic ↔ OpenAI Chat Completions 格式转换
        // - "openai_responses": 需要 Anthropic ↔ OpenAI Responses API 格式转换
        matches!(
            self.get_api_format(provider),
            "openai_chat" | "openai_responses" | "gemini_native"
        )
    }

    fn transform_request(
        &self,
        body: serde_json::Value,
        provider: &Provider,
    ) -> Result<serde_json::Value, ProxyError> {
        transform_claude_request_for_api_format(
            body,
            provider,
            self.get_api_format(provider),
            None,
            None,
        )
    }

    fn transform_response(&self, body: serde_json::Value) -> Result<serde_json::Value, ProxyError> {
        // Heuristic: detect response format by presence of top-level fields.
        // The ProviderAdapter trait's transform_response doesn't receive the Provider
        // config, so we can't check api_format here. Instead we rely on the fact that
        // Responses API always returns "output" while Chat Completions returns "choices".
        // This is safe because the two formats are structurally disjoint.
        if body.get("candidates").is_some() || body.get("promptFeedback").is_some() {
            super::transform_gemini::gemini_to_anthropic(body)
        } else if body.get("output").is_some() {
            super::transform_responses::responses_to_anthropic(body)
        } else {
            super::transform::openai_to_anthropic(body)
        }
    }
}

//! Provider Adapters Module
//!
//! 供应商适配器模块，提供统一的接口抽象不同上游供应商的处理逻辑。
//!
//! ## 模块结构
//! - `adapter`: 定义 `ProviderAdapter` trait
//! - `auth`: 认证类型和策略
//! - `claude`: Claude (Anthropic) 适配器
//! - `codex`: Codex (OpenAI) 适配器
//! - `gemini`: Gemini (Google) 适配器
//! - `models`: API 数据模型
//! - `transform`: 格式转换

mod adapter;
mod auth;
mod claude;
mod claude_reasoning;
mod codex;
pub(crate) mod codex_chat_common;
pub mod codex_chat_history;
mod codex_chat_tool_history;
pub(crate) mod codex_compaction;
pub(crate) mod codex_responses_sse;
mod gemini;
pub(crate) mod gemini_schema;
pub mod gemini_shadow;
pub(crate) mod inline_think;
pub mod models;
pub(crate) mod reasoning_bridge;
pub(crate) mod responses_late_arguments;
pub mod streaming;
pub mod streaming_codex_anthropic;
pub mod streaming_codex_chat;
pub mod streaming_gemini;
pub mod streaming_responses;
pub mod streaming_responses_chat;
pub mod transform;
pub mod transform_chat_responses;
pub mod transform_codex_anthropic;
pub mod transform_codex_chat;
pub mod transform_codex_chat_moonshot_schema;
pub mod transform_codex_responses_namespace;
pub mod transform_codex_responses_xai_sanitize;
pub mod transform_gemini;
pub mod transform_responses;

use crate::app_config::AppType;
use crate::provider::Provider;
use serde::{Deserialize, Serialize};



// 公开导出
pub use adapter::ProviderAdapter;
pub use auth::{AuthInfo, AuthStrategy};
pub use claude::{
    claude_api_format_needs_transform, get_claude_api_format,
    normalize_anthropic_messages_for_provider, transform_claude_request_for_api_format,
    ClaudeAdapter,
};
pub use codex::CodexAdapter;
pub use codex::{
    apply_codex_chat_upstream_model, apply_codex_upstream_model, codex_provider_upstream_model,
    inject_codex_chat_prompt_cache_key, is_codex_official_provider,
    provider_needs_responses_late_arguments_repair, provider_needs_responses_namespace_flatten,
    resolve_codex_chat_reasoning_config,
};
pub use gemini::GeminiAdapter;

/// 供应商类型枚举
///
/// 区分不同供应商的具体实现方式，决定认证和请求处理逻辑。
/// 比 AppType 更细粒度，支持同一 AppType 下的多种变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// Anthropic 官方 API (x-api-key + anthropic-version)
    Claude,
    /// Claude 中转服务 (仅 Bearer 认证，无 x-api-key)
    ClaudeAuth,
    /// OpenAI Codex Response API
    Codex,
    /// Google Gemini API (x-goog-api-key)
    Gemini,
    /// Google Gemini CLI (OAuth Bearer)
    GeminiCli,
    /// OpenRouter（已支持 Anthropic 兼容接口，默认透传）
    OpenRouter,
}

impl ProviderType {
    /// 是否需要格式转换
    ///
    /// 过去 OpenRouter 需要将 Anthropic 格式转换为 OpenAI 格式；
    /// 现在默认关闭转换（因为 OpenRouter 已支持 Claude Code 兼容接口）。
    /// GitHub Copilot 需要转换（Anthropic → OpenAI 格式）。
    #[allow(dead_code)]
    pub fn needs_transform(&self) -> bool {
        matches!(self, ProviderType::Codex | ProviderType::Gemini | ProviderType::GeminiCli)
    }

    /// 获取默认端点
    #[allow(dead_code)]
    pub fn default_endpoint(&self) -> &'static str {
        match self {
            ProviderType::Claude | ProviderType::ClaudeAuth => "https://api.anthropic.com",
            ProviderType::Codex => "https://api.openai.com",
            ProviderType::Gemini | ProviderType::GeminiCli => {
                "https://generativelanguage.googleapis.com"
            }
            ProviderType::OpenRouter => "https://openrouter.ai/api",
        }
    }

    /// 从 AppType 和 Provider 配置推断供应商类型
    ///
    /// 根据配置中的 base_url、auth_mode、api_key 格式等信息推断具体的供应商类型
    #[allow(dead_code)]
    pub fn from_app_type_and_config(app_type: &AppType, provider: &Provider) -> Option<Self> {
        let provider_type = match app_type {
            AppType::Claude => {
                // 供应商若显式选择 Gemini 原生协议，则走 Gemini 适配器。
                if get_claude_api_format(provider) == "gemini_native" {
                    return Some(ProviderType::Gemini);
                }

                let adapter = ClaudeAdapter::new();
                if let Ok(base_url) = adapter.extract_base_url(provider) {
                    if base_url.contains("openrouter.ai") {
                        return Some(ProviderType::OpenRouter);
                    }
                }
                // 仅 Bearer 认证的中转服务
                if provider
                    .settings_config
                    .get("auth_mode")
                    .and_then(|v| v.as_str())
                    == Some("bearer_only")
                {
                    return Some(ProviderType::ClaudeAuth);
                }
                if provider
                    .settings_config
                    .get("env")
                    .and_then(|env| env.get("AUTH_MODE"))
                    .and_then(|v| v.as_str())
                    == Some("bearer_only")
                {
                    return Some(ProviderType::ClaudeAuth);
                }
                ProviderType::Claude
            }
        };
        Some(provider_type)
    }

    /// 转换为字符串表示
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderType::Claude => "claude",
            ProviderType::ClaudeAuth => "claude_auth",
            ProviderType::Codex => "codex",
            ProviderType::Gemini => "gemini",
            ProviderType::GeminiCli => "gemini_cli",
            ProviderType::OpenRouter => "openrouter",
        }
    }
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for ProviderType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "claude" => Ok(ProviderType::Claude),
            "claude_auth" | "claude-auth" => Ok(ProviderType::ClaudeAuth),
            "codex" => Ok(ProviderType::Codex),
            "gemini" => Ok(ProviderType::Gemini),
            "gemini_cli" | "gemini-cli" => Ok(ProviderType::GeminiCli),
            "openrouter" => Ok(ProviderType::OpenRouter),
            _ => Err(format!("Invalid provider type: {s}")),
        }
    }
}

/// 根据 AppType 获取对应的适配器
pub fn get_adapter(app_type: &AppType) -> Option<Box<dyn ProviderAdapter>> {
    Some(match app_type {
        AppType::Claude => Box::new(ClaudeAdapter::new()),
    })
}

/// 根据 ProviderType 获取对应的适配器
#[allow(dead_code)]
pub fn get_adapter_for_provider_type(provider_type: &ProviderType) -> Box<dyn ProviderAdapter> {
    match provider_type {
        ProviderType::Claude
        | ProviderType::ClaudeAuth
        | ProviderType::OpenRouter => Box::new(ClaudeAdapter::new()),
        ProviderType::Codex => Box::new(CodexAdapter::new()),
        ProviderType::Gemini | ProviderType::GeminiCli => Box::new(GeminiAdapter::new()),
    }
}

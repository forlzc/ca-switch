//! Authentication Types
//!
//! 定义认证信息和认证策略，支持多种上游供应商的认证方式。

/// 认证信息
///
/// 包含 API Key 和对应的认证策略
#[derive(Debug, Clone)]
pub struct AuthInfo {
    /// API Key
    pub api_key: String,
    /// 认证策略
    pub strategy: AuthStrategy,
    /// OAuth access_token（用于 GoogleOAuth 策略）
    pub access_token: Option<String>,
}

impl AuthInfo {
    /// 创建新的认证信息
    pub fn new(api_key: String, strategy: AuthStrategy) -> Self {
        Self {
            api_key,
            strategy,
            access_token: None,
        }
    }

    /// 创建带有 access_token 的认证信息（用于 OAuth）
    pub fn with_access_token(api_key: String, access_token: String) -> Self {
        Self {
            api_key,
            strategy: AuthStrategy::GoogleOAuth,
            access_token: Some(access_token),
        }
    }

    /// 返回遮蔽后的 API Key（用于日志输出）
    ///
    /// 显示前4位和后4位，中间用 `...` 代替
    /// 如果 key 长度不足8位，则返回 `***`
    #[allow(dead_code)]
    pub fn masked_key(&self) -> String {
        if self.api_key.chars().count() > 8 {
            let prefix: String = self.api_key.chars().take(4).collect();
            let suffix: String = self
                .api_key
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{prefix}...{suffix}")
        } else {
            "***".to_string()
        }
    }

    /// 返回遮蔽后的 access_token（用于日志输出）
    #[allow(dead_code)]
    pub fn masked_access_token(&self) -> Option<String> {
        self.access_token.as_ref().map(|token| {
            if token.chars().count() > 8 {
                let prefix: String = token.chars().take(4).collect();
                let suffix: String = token
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                format!("{prefix}...{suffix}")
            } else {
                "***".to_string()
            }
        })
    }
}

/// 认证策略
///
/// 不同供应商使用不同的认证方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStrategy {
    /// Anthropic 认证方式
    /// - Header: `x-api-key: <api_key>`
    /// - Header: `anthropic-version: 2023-06-01`
    Anthropic,

    /// Claude 中转服务认证方式（仅 Bearer，无 x-api-key）
    ///
    /// - Header: `Authorization: Bearer <api_key>`
    ///
    /// 用于不支持 x-api-key 的中转服务
    ClaudeAuth,

    /// Bearer Token 认证方式（OpenAI 等）
    ///
    /// - Header: `Authorization: Bearer <api_key>`
    Bearer,

    /// Google API Key 认证方式
    ///
    /// - Header: `x-goog-api-key: <api_key>`
    Google,

    /// Google OAuth 认证方式
    ///
    /// - Header: `Authorization: Bearer <access_token>`
    ///
    /// 用于 Gemini CLI 等需要 OAuth 的场景
    GoogleOAuth,
}

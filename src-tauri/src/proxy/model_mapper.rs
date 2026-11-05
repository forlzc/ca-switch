//! 模型映射模块
//!
//! 在请求转发前，根据 Provider 配置替换请求中的模型名称

use crate::claude_desktop_config::ONE_M_CONTEXT_MARKER;
use crate::provider::Provider;
use serde_json::Value;

/// 模型映射配置。
///
/// 精简后 Claude 入口只保留一个默认模型映射：配了 `ANTHROPIC_MODEL` 就把所有请求
/// 统一改写成它，没配就原样使用请求里的模型名（等同用户直传上游模型名）。
pub struct ModelMapping {
    pub default_model: Option<String>,
}

impl ModelMapping {
    /// 从 Provider 配置中提取默认模型映射
    pub fn from_provider(provider: &Provider) -> Self {
        Self {
            default_model: default_model_from_provider(provider),
        }
    }

    /// 检查是否配置了默认模型映射
    pub fn has_mapping(&self) -> bool {
        self.default_model.is_some()
    }

    /// 根据原始模型名称获取映射后的模型
    pub fn map_model(&self, original_model: &str) -> String {
        if let Some(ref m) = self.default_model {
            return m.clone();
        }

        // 无映射，保持原样：默认使用请求里的模型名
        original_model.to_string()
    }
}

/// 供应商的唯一默认模型。
///
/// 前端只保留一个「默认模型」输入框，落到 `settings_config.env.ANTHROPIC_MODEL`；
/// 它同时会写进 `providers.default_model` 列。留空即透传请求里的模型名。
pub fn default_model_from_provider(provider: &Provider) -> Option<String> {
    provider
        .settings_config
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_MODEL"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Gemini 入站请求的模型名在 URI 里（`/v1beta/models/{model}:generateContent`），
/// 不能只改 body，必须把 URI 的模型段一起换掉。
pub fn rewrite_gemini_endpoint_model(endpoint: &str, model: &str) -> String {
    let Some(idx) = endpoint.find("/models/") else {
        return endpoint.to_string();
    };
    let prefix_end = idx + "/models/".len();
    let rest = &endpoint[prefix_end..];
    let stop = rest.find([':', '?']).unwrap_or(rest.len());

    let mut out = String::with_capacity(endpoint.len());
    out.push_str(&endpoint[..prefix_end]);
    out.push_str(super::gemini_url::normalize_gemini_model_id(model));
    out.push_str(&rest[stop..]);
    out
}

/// 对请求体应用模型映射
///
/// 返回 (映射后的请求体, 原始模型名, 映射后模型名)
pub fn apply_model_mapping(
    mut body: Value,
    provider: &Provider,
) -> (Value, Option<String>, Option<String>) {
    let mapping = ModelMapping::from_provider(provider);

    // 如果没有配置映射，直接返回
    if !mapping.has_mapping() {
        let original = body.get("model").and_then(|m| m.as_str()).map(String::from);
        return (body, original, None);
    }

    // 提取原始模型名
    let original_model = body.get("model").and_then(|m| m.as_str()).map(String::from);

    if let Some(ref original) = original_model {
        let mapped = mapping.map_model(original);

        if mapped != *original {
            log::debug!("[ModelMapper] 模型映射: {original} → {mapped}");
            body["model"] = serde_json::json!(mapped);
            return (body, Some(original.clone()), Some(mapped));
        }
    }

    (body, original_model, None)
}

/// Claude Code 通过 `[1M]` 后缀声明 100 万上下文能力；上游 API
/// 通常不接受这个本地能力标记，转发前需要剥离。
pub fn strip_one_m_suffix_for_upstream(model: &str) -> &str {
    let trimmed = model.trim_end();
    let marker = ONE_M_CONTEXT_MARKER.as_bytes();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= marker.len()
        && bytes[bytes.len() - marker.len()..].eq_ignore_ascii_case(marker)
    {
        return trimmed[..trimmed.len() - marker.len()].trim_end();
    }
    model
}

pub fn strip_one_m_suffix_for_upstream_from_body(mut body: Value) -> Value {
    let Some(model) = body.get("model").and_then(Value::as_str) else {
        return body;
    };

    let stripped = strip_one_m_suffix_for_upstream(model);
    if stripped != model {
        log::debug!("[ModelMapper] 去除本地 1M 标记: {model} → {stripped}");
        body["model"] = serde_json::json!(stripped);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_provider_with_mapping() -> Provider {
        Provider {
            id: "test".to_string(),
            name: "Test".to_string(),
            settings_config: json!({
                "env": {
                    "ANTHROPIC_MODEL": "default-model",
                }
            }),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn create_provider_without_mapping() -> Provider {
        Provider {
            id: "test".to_string(),
            name: "Test".to_string(),
            settings_config: json!({}),
            website_url: None,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    #[test]
    fn any_request_model_uses_the_single_default() {
        let provider = create_provider_with_mapping();
        for model in [
            "claude-sonnet-4-5-20250929",
            "claude-haiku-4-5",
            "claude-opus-4-5",
            "some-unknown-model",
        ] {
            let body = json!({ "model": model });
            let (result, original, mapped) = apply_model_mapping(body, &provider);
            assert_eq!(result["model"], "default-model", "{model}");
            assert_eq!(original, Some(model.to_string()));
            assert_eq!(mapped, Some("default-model".to_string()));
        }
    }

    #[test]
    fn thinking_does_not_affect_model_mapping() {
        let provider = create_provider_with_mapping();
        let body = json!({
            "model": "claude-sonnet-4-5",
            "thinking": {"type": "enabled"}
        });
        let (result, _, mapped) = apply_model_mapping(body, &provider);
        assert_eq!(result["model"], "default-model");
        assert_eq!(mapped, Some("default-model".to_string()));
    }

    #[test]
    fn no_mapping_keeps_request_model() {
        let provider = create_provider_without_mapping();
        let body = json!({"model": "claude-sonnet-4-5"});
        let (result, original, mapped) = apply_model_mapping(body, &provider);
        assert_eq!(result["model"], "claude-sonnet-4-5");
        assert_eq!(original, Some("claude-sonnet-4-5".to_string()));
        assert!(mapped.is_none());
    }

    #[test]
    fn strips_one_m_suffix_before_upstream() {
        let body = json!({"model": "deepseek-v4-pro[1M]"});
        let result = strip_one_m_suffix_for_upstream_from_body(body);
        assert_eq!(result["model"], "deepseek-v4-pro");
    }

    #[test]
    fn strips_one_m_suffix_after_mapping() {
        let mut provider = create_provider_with_mapping();
        provider.settings_config = json!({
            "env": {
                "ANTHROPIC_MODEL": "deepseek-v4-pro [1M]"
            }
        });

        let body = json!({"model": "claude-sonnet-4-6"});
        let (mapped, _, _) = apply_model_mapping(body, &provider);
        let result = strip_one_m_suffix_for_upstream_from_body(mapped);

        assert_eq!(result["model"], "deepseek-v4-pro");
    }

    #[test]
    fn gemini_endpoint_model_is_rewritten_but_action_and_query_are_kept() {
        assert_eq!(
            rewrite_gemini_endpoint_model(
                "/v1beta/models/gemini-2.0-flash:streamGenerateContent?alt=sse",
                "default-model",
            ),
            "/v1beta/models/default-model:streamGenerateContent?alt=sse"
        );
        assert_eq!(
            rewrite_gemini_endpoint_model("/v1beta/models/gemini-pro", "models/default-model"),
            "/v1beta/models/default-model"
        );
        // 非 Gemini 形状的路径原样返回
        assert_eq!(
            rewrite_gemini_endpoint_model("/v1/chat/completions", "default-model"),
            "/v1/chat/completions"
        );
    }

    #[test]
    fn default_model_is_trimmed_and_blank_counts_as_unset() {
        let mut provider = create_provider_with_mapping();
        provider.settings_config = json!({ "env": { "ANTHROPIC_MODEL": "  deepseek-v4-pro  " } });
        assert_eq!(
            default_model_from_provider(&provider).as_deref(),
            Some("deepseek-v4-pro")
        );

        provider.settings_config = json!({ "env": { "ANTHROPIC_MODEL": "   " } });
        assert_eq!(default_model_from_provider(&provider), None);
    }

    #[test]
    fn keeps_model_without_one_m_suffix() {
        let body = json!({"model": "deepseek-v4-pro"});
        let result = strip_one_m_suffix_for_upstream_from_body(body);
        assert_eq!(result["model"], "deepseek-v4-pro");
    }
}

//! 协议模型：caswitch 唯一的「上游」维度。
//!
//! 原项目按 14 个客户端（`AppType`）组织一切——供应商、代理参数、熔断器、
//! 会话统计全挂在 `app_type` 上。caswitch 不再写任何客户端配置，客户端身份
//! 只剩一个作用：它连的是哪个端口/路径。真正决定请求怎么发出、响应怎么解析的
//! 只有协议。
//!
//! 本模块给出四种协议、上游鉴权方式，以及「入口协议 × 上游协议」的转换矩阵。
//!
//! 落地节奏：2a 只引入类型与矩阵并用测试钉死现状，调用点仍走 `AppType`，
//! 行为零变化；2c 起把 `forwarder` 里散落的判定逐个换成 [`Matrix::plan`]。

use crate::app_config::AppType;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// 协议。既描述入站入口，也描述上游。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol {
    /// Anthropic Messages API：`/v1/messages`
    #[serde(rename = "anthropic")]
    Anthropic,
    /// OpenAI Chat Completions：`/v1/chat/completions`
    #[serde(rename = "openai_chat")]
    OpenAiChat,
    /// OpenAI Responses API：`/v1/responses`
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    /// Gemini generateContent：`/v1beta/models/{model}:generateContent`
    #[serde(rename = "gemini")]
    Gemini,
}

impl Protocol {
    pub const ALL: [Protocol; 4] = [
        Protocol::Anthropic,
        Protocol::OpenAiChat,
        Protocol::OpenAiResponses,
        Protocol::Gemini,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Protocol::Anthropic => "anthropic",
            Protocol::OpenAiChat => "openai_chat",
            Protocol::OpenAiResponses => "openai_responses",
            Protocol::Gemini => "gemini",
        }
    }

    /// 兼容旧 `ProviderMeta::api_format` 的取值。
    ///
    /// 旧值域是 `anthropic | openai_chat | openai_responses | gemini_native`，
    /// 迁移时唯一需要改名的是 `gemini_native`；另接受 `chat` / `responses`
    /// 两个简写，方便 deep link 与手写配置。
    pub fn from_api_format(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Some(Protocol::Anthropic),
            "openai_chat" | "chat" => Some(Protocol::OpenAiChat),
            "openai_responses" | "responses" => Some(Protocol::OpenAiResponses),
            "gemini" | "gemini_native" => Some(Protocol::Gemini),
            _ => None,
        }
    }

    /// 迁移兜底：没有任何显式证据时，按原客户端家族推定。
    ///
    /// 只在 `api_format` 与 `wire_api` 都读不到时才用得上，所以这里宁可按
    /// 家族惯例，不做 base_url 嗅探（那属于迁移脚本的第二级证据）。
    ///
    /// 判定依据：
    /// - anthropic 家族：`Claude` / `Claude Desktop` 原生就是 Messages API。
    /// - responses 家族：`Codex` 与 `GrokBuild` 的 wire_api 缺省即 responses；
    ///   注意若供应商配置里有 `wire_api = "chat"`，迁移脚本会优先采信它。
    /// - gemini 家族：`Gemini` 原生；`Agy` 的投影读的就是
    ///   `GOOGLE_GEMINI_BASE_URL`（`agy_config.rs:113`），同一协议。
    /// - 其余（openai 兼容的本地工具）：按 chat 处理。
    pub fn from_app_type(app: &AppType) -> Self {
        match app {
            AppType::Claude | AppType::ClaudeDesktop => Protocol::Anthropic,
            AppType::Codex | AppType::GrokBuild => Protocol::OpenAiResponses,
            AppType::Gemini | AppType::Agy => Protocol::Gemini,
            AppType::OpenCode
            | AppType::OpenClaw
            | AppType::Hermes
            | AppType::Pi
            | AppType::Mcode
            | AppType::Kimi
            | AppType::Dsh
            | AppType::Omp => Protocol::OpenAiChat,
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Protocol {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Protocol::from_api_format(s).ok_or(())
    }
}

/// 协议判定的证据强度。迁移时落进 `providers.protocol_source`，
/// `Guessed` 的供应商在列表里要打角标让用户确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolSource {
    /// 用户显式选过（`meta.apiFormat`）。
    Explicit,
    /// 从配置内容读出来的可靠证据（codex `wire_api`、端点后缀、家族唯一）。
    Inferred,
    /// 没有任何证据，按家族惯例推的，需要确认。
    Guessed,
}

/// 上游鉴权方式。
///
/// 与协议正交：同一个 responses 协议，既可能是 Bearer API key，也可能是
/// Codex 的 OAuth 令牌。凭据一律存在 caswitch 自己的数据目录，不读也不写
/// 任何 agent 的 `auth.json`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuthStyle {
    /// 静态 API key（Bearer 或 `x-api-key`）。
    #[serde(rename = "api_key")]
    ApiKey,
    /// Codex OAuth（`data_dir/codex_oauth_auth.json`）。
    #[serde(rename = "codex_oauth")]
    CodexOauth,
    /// xAI OAuth。
    #[serde(rename = "xai_oauth")]
    XaiOauth,
    /// GitHub Copilot 托管鉴权。
    #[serde(rename = "copilot")]
    Copilot,
    /// 无需鉴权（本地模型、内网网关）。
    #[serde(rename = "none")]
    None,
}

impl AuthStyle {
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthStyle::ApiKey => "api_key",
            AuthStyle::CodexOauth => "codex_oauth",
            AuthStyle::XaiOauth => "xai_oauth",
            AuthStyle::Copilot => "copilot",
            AuthStyle::None => "none",
        }
    }

    pub fn from_str_opt(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "api_key" | "apikey" => Some(AuthStyle::ApiKey),
            "codex_oauth" => Some(AuthStyle::CodexOauth),
            "xai_oauth" => Some(AuthStyle::XaiOauth),
            "copilot" | "github_copilot" => Some(AuthStyle::Copilot),
            "none" => Some(AuthStyle::None),
            _ => None,
        }
    }
}

impl Default for AuthStyle {
    fn default() -> Self {
        AuthStyle::ApiKey
    }
}

impl fmt::Display for AuthStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一格的转换需要哪两个函数。
///
/// 一格「通」必须是成对的：请求方向（入口请求 → 上游请求）改写，
/// 加响应方向（上游响应 → 入口响应）改写。只数「有没有一个 transform 文件」
/// 会虚高——历史规划里就因为这个数错过一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transform {
    /// 请求方向改写函数名，便于 2c 接线时对照。
    pub request: &'static str,
    /// 响应方向改写函数名（含 SSE 路径）。
    pub response: &'static str,
    /// 现有生产接线位置。
    pub wiring: &'static str,
}

/// 入口协议 × 上游协议 的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conversion {
    /// 同协议，原样转发。
    Passthrough,
    /// 需要双向改写。
    Convert(Transform),
    /// 没有可用转换器。调用方必须返回 501 并说明原因，禁止静默降级。
    Unsupported,
}

/// 转换矩阵。
pub struct Matrix;

impl Matrix {
    /// 判断一条请求该怎么处理。
    ///
    /// `inbound` 由入口路径决定，`upstream` 由当前激活供应商的协议决定。
    /// 全局单激活是前提：只有一个供应商生效，四条入口都打到它，才可能出现
    /// 入口与上游协议不一致、需要转换的情况。
    pub fn plan(inbound: Protocol, upstream: Protocol) -> Conversion {
        use Protocol::{Anthropic, Gemini, OpenAiChat, OpenAiResponses};

        if inbound == upstream {
            return Conversion::Passthrough;
        }

        match (inbound, upstream) {
            // Claude Code 入口对三种上游全通——这是原项目做得最厚的一条路径，
            // 也是「Gemini 供应商能用 Claude Code 跑」的依据。
            (Anthropic, OpenAiChat) => Conversion::Convert(Transform {
                request: "anthropic_to_openai_with_reasoning_content",
                response: "openai_to_anthropic",
                wiring: "proxy/providers/claude.rs:456",
            }),
            (Anthropic, OpenAiResponses) => Conversion::Convert(Transform {
                request: "anthropic_to_responses",
                response: "responses_to_anthropic",
                wiring: "proxy/providers/claude.rs（openai_responses 分支）",
            }),
            (Anthropic, Gemini) => Conversion::Convert(Transform {
                request: "anthropic_to_gemini",
                response: "gemini_to_anthropic",
                wiring: "proxy/providers/claude.rs（gemini_native 分支）",
            }),
            // Codex 入口：上游可以是 chat 或 anthropic，不能是 gemini。
            (OpenAiResponses, OpenAiChat) => Conversion::Convert(Transform {
                request: "responses_to_chat_completions",
                response: "chat_completion_to_response",
                wiring: "proxy/handlers.rs handle_codex_chat_to_responses_transform",
            }),
            (OpenAiResponses, Anthropic) => Conversion::Convert(Transform {
                request: "responses_request_to_anthropic",
                response: "anthropic_response_to_responses",
                wiring: "proxy/providers/transform_codex_anthropic.rs",
            }),
            // 其余 7 格没有转换器。需求 4（chat 入口 → responses 上游）就在这里，
            // 由 P4 补 chat_request_to_responses + responses_response_to_chat。
            _ => Conversion::Unsupported,
        }
    }

    /// 当前所有不支持的格子，按入口协议分组。UI 的能力提示与 501 文案都从
    /// 这里取，避免前后端各写一份表。
    pub fn unsupported_cells() -> Vec<(Protocol, Protocol)> {
        let mut out = Vec::new();
        for inbound in Protocol::ALL {
            for upstream in Protocol::ALL {
                if matches!(Matrix::plan(inbound, upstream), Conversion::Unsupported) {
                    out.push((inbound, upstream));
                }
            }
        }
        out
    }
}

/// 从供应商配置推断协议。
///
/// caswitch 不再有"每客户端一个默认协议"：协议是供应商自身的属性。历史数据里
/// 没有这一列，只能按证据链推。证据从强到弱四级，命中即返回：
///
/// 1. `meta.apiFormat`（用户当年显式选过的格式）
/// 2. codex TOML 里的 `wire_api`
/// 3. `settings_config` 里 base URL 的端点后缀
/// 4. 家族惯例兜底
///
/// 推断结果同时给出证据强度，`Guessed` 需要在首屏让用户确认。
pub mod infer {
    use super::{Protocol, ProtocolSource};
    use crate::app_config::AppType;
    use crate::provider::Provider;
    use serde_json::Value;
    use toml::Value as TomlValue;

    /// 读 codex 供应商 TOML 里的 `wire_api`。
    ///
    /// 优先读 `model_provider` 指向的那个 `[model_providers.*]`，读不到再退回顶层
    /// `wire_api`。原实现在 `proxy/providers/codex.rs`，为保证代理热路径与迁移
    /// 脚本用同一份判定，移到这里集中维护。
    pub(crate) fn codex_wire_api_from_toml(config_text: &str) -> Option<String> {
        let doc = config_text.parse::<TomlValue>().ok()?;

        if let Some(active_provider) = doc.get("model_provider").and_then(|v| v.as_str()) {
            if let Some(wire_api) = doc
                .get("model_providers")
                .and_then(|providers| providers.get(active_provider))
                .and_then(|provider| provider.get("wire_api"))
                .and_then(|v| v.as_str())
            {
                return Some(wire_api.to_string());
            }
        }

        doc.get("wire_api")
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
    }

    /// 推断某个供应商的协议。
    ///
    /// `app` 只在最后一级兜底时用到——协议化之后运行期没有 `AppType`，
    /// 迁移脚本会把它作为"原客户端家族"的线索传进来。
    pub fn of_provider(app: &AppType, provider: &Provider) -> (Protocol, ProtocolSource) {
        // 1. 显式选择
        if let Some(raw) = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.api_format.as_deref())
        {
            if let Some(protocol) = Protocol::from_api_format(raw) {
                return (protocol, ProtocolSource::Explicit);
            }
        }

        // 2. codex TOML 的 wire_api
        if let Some(config_text) = provider
            .settings_config
            .get("config")
            .and_then(|value| value.as_str())
        {
            if let Some(wire_api) = codex_wire_api_from_toml(config_text) {
                if let Some(protocol) = Protocol::from_api_format(&wire_api) {
                    return (protocol, ProtocolSource::Inferred);
                }
            }
        }

        // 3. 端点后缀
        if let Some(protocol) = protocol_from_endpoint_markers(&provider.settings_config) {
            return (protocol, ProtocolSource::Inferred);
        }

        // 4. 家族惯例
        let protocol = Protocol::from_app_type(app);
        let source = match protocol {
            // Claude Code 只会说 Anthropic，Gemini 家族只会说 Gemini，这两条几乎是定论
            Protocol::Anthropic | Protocol::Gemini => ProtocolSource::Inferred,
            // codex 缺 wire_api、以及 openai 兼容工具族，都可能是 chat 也可能是 responses
            Protocol::OpenAiChat | Protocol::OpenAiResponses => ProtocolSource::Guessed,
        };
        (protocol, source)
    }

    /// 从 `settings_config` 里的 base URL 找端点证据。
    ///
    /// 只认带端点路径的 URL：`https://relay.example.com` 这种纯 origin 推不出协议
    /// （同一个 origin 既有 chat 也有 responses），必须留给下一级兜底。
    fn protocol_from_endpoint_markers(settings: &Value) -> Option<Protocol> {
        let mut urls = Vec::new();
        collect_base_urls(settings, &mut urls);
        urls.iter().find_map(|url| protocol_from_url(url))
    }

    fn collect_base_urls(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, val) in map {
                    let lower = key.to_ascii_lowercase();
                    // 兼容 `base_url` / `baseUrl` / `ANTHROPIC_BASE_URL` /
                    // `GOOGLE_GEMINI_BASE_URL` 这些写法
                    if matches!(
                        lower.as_str(),
                        "base_url" | "baseurl" | "api_base" | "api_endpoint"
                    ) || lower.ends_with("_base_url")
                        || lower.ends_with("_baseurl")
                    {
                        if let Some(text) = val.as_str() {
                            out.push(text.to_string());
                        }
                    }
                    collect_base_urls(val, out);
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect_base_urls(item, out);
                }
            }
            _ => {}
        }
    }

    fn protocol_from_url(url: &str) -> Option<Protocol> {
        let url = url.trim().trim_end_matches('/').to_ascii_lowercase();

        // Gemini 的路径带 version 段，且模型名在路径里，没有统一的端点尾巴
        if url.contains("/v1beta") {
            return Some(Protocol::Gemini);
        }
        if url.ends_with("/v1/messages") || url.ends_with("/messages") {
            return Some(Protocol::Anthropic);
        }
        if url.ends_with("/v1/responses") || url.ends_with("/responses") {
            return Some(Protocol::OpenAiResponses);
        }
        if url.ends_with("/v1/chat/completions") || url.ends_with("/chat/completions") {
            return Some(Protocol::OpenAiChat);
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::provider::ProviderMeta;
        use serde_json::json;

        fn provider(settings: Value, api_format: Option<&str>) -> Provider {
            Provider {
                id: "p".to_string(),
                name: "p".to_string(),
                settings_config: settings,
                website_url: None,
                category: None,
                created_at: None,
                sort_index: None,
                notes: None,
                meta: api_format.map(|value| ProviderMeta {
                    api_format: Some(value.to_string()),
                    ..Default::default()
                }),
                icon: None,
                icon_color: None,
                in_failover_queue: false,
            }
        }

        #[test]
        fn explicit_api_format_wins_over_everything_else() {
            // URL 看着像 anthropic，但用户显式选了 openai_chat
            let p = provider(
                json!({ "env": { "ANTHROPIC_BASE_URL": "https://relay.example.com/v1/messages" } }),
                Some("openai_chat"),
            );
            assert_eq!(
                of_provider(&AppType::Claude, &p),
                (Protocol::OpenAiChat, ProtocolSource::Explicit)
            );
        }

        #[test]
        fn codex_wire_api_is_read_from_the_active_provider_table() {
            let toml = r#"
model_provider = "relay"
model = "gpt-5"

[model_providers.relay]
base_url = "https://relay.example.com/v1"
wire_api = "chat"
"#;
            let p = provider(json!({ "config": toml }), None);
            assert_eq!(
                of_provider(&AppType::Codex, &p),
                (Protocol::OpenAiChat, ProtocolSource::Inferred)
            );
        }

        #[test]
        fn codex_wire_api_anthropic_is_recognised() {
            let toml =
                "model_provider = \"relay\"\n\n[model_providers.relay]\nwire_api = \"anthropic\"\n";
            let p = provider(json!({ "config": toml }), None);
            assert_eq!(
                of_provider(&AppType::Codex, &p),
                (Protocol::Anthropic, ProtocolSource::Inferred)
            );
        }

        #[test]
        fn endpoint_suffix_decides_when_there_is_no_explicit_format() {
            let p = provider(
                json!({ "env": { "OPENAI_BASE_URL": "https://relay.example.com/v1/responses" } }),
                None,
            );
            assert_eq!(
                of_provider(&AppType::OpenClaw, &p),
                (Protocol::OpenAiResponses, ProtocolSource::Inferred)
            );
        }

        #[test]
        fn gemini_endpoint_is_recognised_by_version_segment() {
            let p = provider(
                json!({ "env": { "GOOGLE_GEMINI_BASE_URL": "https://gw.example.com/v1beta" } }),
                None,
            );
            assert_eq!(
                of_provider(&AppType::Agy, &p),
                (Protocol::Gemini, ProtocolSource::Inferred)
            );
        }

        #[test]
        fn bare_origin_falls_back_to_family() {
            // 纯 origin 推不出协议，只能按家族
            let p = provider(
                json!({ "env": { "ANTHROPIC_BASE_URL": "https://api.anthropic.com" } }),
                None,
            );
            assert_eq!(
                of_provider(&AppType::Claude, &p),
                (Protocol::Anthropic, ProtocolSource::Inferred)
            );
        }

        #[test]
        fn codex_without_wire_api_is_flagged_as_guess() {
            let p = provider(json!({ "env": {} }), None);
            assert_eq!(
                of_provider(&AppType::Codex, &p),
                (Protocol::OpenAiResponses, ProtocolSource::Guessed)
            );
        }

        #[test]
        fn openai_compatible_tools_default_to_chat_and_are_flagged() {
            let p = provider(json!({ "base_url": "https://relay.example.com" }), None);
            assert_eq!(
                of_provider(&AppType::OpenCode, &p),
                (Protocol::OpenAiChat, ProtocolSource::Guessed)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_strings_roundtrip() {
        for p in Protocol::ALL {
            assert_eq!(Protocol::from_str(p.as_str()), Ok(p));
        }
        // 旧值 gemini_native 必须还能读
        assert_eq!(
            Protocol::from_api_format("gemini_native"),
            Some(Protocol::Gemini)
        );
        assert_eq!(
            Protocol::from_api_format(" GEMINI "),
            Some(Protocol::Gemini)
        );
        assert_eq!(Protocol::from_api_format("openai"), None);
    }

    #[test]
    fn protocol_json_matches_db_values() {
        // 落库字符串必须与 CHECK 约束、前端 i18n key 一致
        assert_eq!(
            serde_json::to_string(&Protocol::OpenAiChat).unwrap(),
            "\"openai_chat\""
        );
        assert_eq!(
            serde_json::from_str::<Protocol>("\"openai_responses\"").unwrap(),
            Protocol::OpenAiResponses
        );
    }

    #[test]
    fn auth_style_strings_roundtrip() {
        let all = [
            AuthStyle::ApiKey,
            AuthStyle::CodexOauth,
            AuthStyle::XaiOauth,
            AuthStyle::Copilot,
            AuthStyle::None,
        ];
        for a in all {
            assert_eq!(AuthStyle::from_str_opt(a.as_str()), Some(a));
        }
        assert_eq!(AuthStyle::default(), AuthStyle::ApiKey);
    }

    /// 12 格 = 4×4 − 4 个同协议。这张表是唯一权威：以后谁动了转换函数，
    /// 这里会红，不允许再靠记忆判断「有没有转换器」。
    #[test]
    fn matrix_covers_exactly_five_cells() {
        use Protocol::*;
        let expected_convert = [
            (Anthropic, OpenAiChat),
            (Anthropic, OpenAiResponses),
            (Anthropic, Gemini),
            (OpenAiResponses, OpenAiChat),
            (OpenAiResponses, Anthropic),
        ];

        let mut passthrough = 0;
        let mut convert = Vec::new();
        let mut unsupported = Vec::new();
        for inbound in Protocol::ALL {
            for upstream in Protocol::ALL {
                match Matrix::plan(inbound, upstream) {
                    Conversion::Passthrough => passthrough += 1,
                    Conversion::Convert(_) => convert.push((inbound, upstream)),
                    Conversion::Unsupported => unsupported.push((inbound, upstream)),
                }
            }
        }

        assert_eq!(passthrough, 4, "同协议四格应为透传");
        assert_eq!(convert.len(), 5, "现网只接了 5 个转换方向");
        assert_eq!(unsupported.len(), 7, "12 - 4 - 5");
        for cell in expected_convert {
            assert!(convert.contains(&cell), "缺了 {cell:?} 的转换接线");
        }
        assert_eq!(Matrix::unsupported_cells().len(), 7);
    }

    /// 需求 4 落在哪一格，用断言固定下来，避免再靠口头记忆。
    #[test]
    fn chat_entry_to_responses_upstream_is_the_p4_gap() {
        assert_eq!(
            Matrix::plan(Protocol::OpenAiChat, Protocol::OpenAiResponses),
            Conversion::Unsupported
        );
        // 反向（responses 入口 → chat 上游）现网是通的
        assert!(matches!(
            Matrix::plan(Protocol::OpenAiResponses, Protocol::OpenAiChat),
            Conversion::Convert(_)
        ));
    }

    /// 每条 Convert 都必须给出两个方向的函数名与接线位置，否则 2c 会接错。
    #[test]
    fn every_convert_cell_names_both_directions() {
        for (inbound, upstream) in [
            (Protocol::Anthropic, Protocol::OpenAiChat),
            (Protocol::Anthropic, Protocol::OpenAiResponses),
            (Protocol::Anthropic, Protocol::Gemini),
            (Protocol::OpenAiResponses, Protocol::OpenAiChat),
            (Protocol::OpenAiResponses, Protocol::Anthropic),
        ] {
            match Matrix::plan(inbound, upstream) {
                Conversion::Convert(t) => {
                    assert!(!t.request.is_empty());
                    assert!(!t.response.is_empty());
                    assert!(!t.wiring.is_empty());
                }
                other => panic!("{inbound:?}→{upstream:?} 应为 Convert，实际 {other:?}"),
            }
        }
    }

    #[test]
    fn app_type_family_fallback() {
        assert_eq!(
            Protocol::from_app_type(&AppType::Claude),
            Protocol::Anthropic
        );
        assert_eq!(
            Protocol::from_app_type(&AppType::ClaudeDesktop),
            Protocol::Anthropic
        );
        assert_eq!(
            Protocol::from_app_type(&AppType::Codex),
            Protocol::OpenAiResponses
        );
        assert_eq!(
            Protocol::from_app_type(&AppType::GrokBuild),
            Protocol::OpenAiResponses
        );
        assert_eq!(Protocol::from_app_type(&AppType::Gemini), Protocol::Gemini);
        assert_eq!(Protocol::from_app_type(&AppType::Agy), Protocol::Gemini);
        assert_eq!(
            Protocol::from_app_type(&AppType::OpenCode),
            Protocol::OpenAiChat
        );
    }
}

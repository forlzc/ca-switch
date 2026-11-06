//! 请求转发器
//!
//! 负责将请求转发到上游Provider，支持故障转移

use super::hyper_client::{ProxyResponse, MAX_RESPONSE_BODY_BYTES};
use super::{
    body_filter::filter_private_params_with_whitelist,
    content_encoding::{decompress_body_with_limit, get_content_encoding},
    error::*,
    failover_switch::FailoverSwitchManager,
    json_canonical::{canonicalize_value, short_value_hash},
    log_codes::fwd as log_fwd,
    opaque_state_rectifier::{
        carries_encrypted_agent_task, rectify_opaque_state, unreadable_agent_task_error,
        OpaqueStateRejection,
    },
    provider_router::ProviderRouter,
    providers::{
        codex_chat_history::CodexChatHistoryStore, gemini_shadow::GeminiShadowStore, get_adapter,
        ProviderAdapter, ProviderType,
    },
    thinking_budget_rectifier::{rectify_thinking_budget, should_rectify_thinking_budget},
    thinking_rectifier::{
        normalize_thinking_type, rectify_anthropic_request, should_rectify_thinking_signature,
    },
    types::{OptimizerConfig, ProxyStatus, RectifierConfig},
    ProxyError,
};
use crate::protocol::{Conversion, Matrix, Protocol};
use crate::{
    app_config::AppType,
    provider::{LocalProxyRequestOverrides, Provider},
};
use bytes::Bytes;
use futures::StreamExt;
use http::Extensions;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 入口协议：由请求路径决定，路径信息不足时才退回客户端家族。
///
/// 同一个客户端入口可能同时提供多种路径（Codex 既有 `/v1/responses`，
/// 也兼容 `/v1/chat/completions`），所以不能只看 `AppType`。
fn inbound_protocol_for(app_type: &AppType, endpoint: &str) -> Protocol {
    let path = endpoint.split('?').next().unwrap_or(endpoint);
    if path.ends_with("/chat/completions") {
        Protocol::OpenAiChat
    } else if path.ends_with("/responses") {
        Protocol::OpenAiResponses
    } else if path.contains("/models/") || path.contains("/v1beta/") {
        Protocol::Gemini
    } else if path.ends_with("/messages") {
        Protocol::Anthropic
    } else {
        Protocol::from_app_type(app_type)
    }
}

/// 供应商的协议。与落库的 `providers.protocol` 使用同一份推断逻辑
/// （`infer::of_provider`），保证「列表角标」「矩阵判定」「迁移」口径一致。
pub(crate) fn provider_protocol(app_type: &AppType, provider: &Provider) -> Protocol {
    crate::protocol::infer::of_provider(app_type, provider).0
}

/// C′：故障转移只允许同协议。
///
/// 返回可尝试的供应商链：首家是链头，后续只保留与链头协议相同的供应商。
/// 跨协议换家会让已经按入口协议改写过的请求打到不兼容的上游，必须跳过。
fn same_protocol_failover_chain<'a>(
    app_type: &AppType,
    providers: &'a [Provider],
) -> Vec<&'a Provider> {
    let Some((first, rest)) = providers.split_first() else {
        return Vec::new();
    };
    let primary = provider_protocol(app_type, first);
    let mut chain = vec![first];
    for provider in rest {
        if provider_protocol(app_type, provider) == primary {
            chain.push(provider);
        } else {
            log::warn!(
                "[{}] [FO-006] 跳过协议不同的故障转移供应商 {}（{} ≠ {}）",
                app_type.as_str(),
                provider.name,
                provider_protocol(app_type, provider).as_str(),
                primary.as_str()
            );
        }
    }
    chain
}

/// Whether the upstream belongs to the OpenCode gateway (`opencode.ai`).
///
/// That gateway (<https://opencode.ai/docs/go/>) expects clients to identify
/// themselves with a `User-Agent` and to carry a **stable per-conversation**
/// session id in `x-opencode-session`; a request without the session header is
/// rejected with `400 MissingSessionID`. Claude Code and Codex already send
/// headers the gateway recognises, Claude Desktop does not - the proxy fills
/// the gap (see the injection site in `forward`).
fn is_opencode_upstream(upstream_host: Option<&str>) -> bool {
    upstream_host
        .map(|host| {
            let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
            host.trim_matches(['[', ']']).to_ascii_lowercase()
        })
        .is_some_and(|host| host == "opencode.ai" || host.ends_with(".opencode.ai"))
}

/// Derive a session id that stays **stable within one conversation**: a digest
/// of the system prompt plus the first user message.
///
/// The gateway uses the session id for upstream routing and prompt caching, so
/// it must not change between the turns of a conversation. Claude Desktop hands
/// CC Switch a fresh id on every request, which defeats that; hashing the
/// conversation prefix keeps the id stable while still separating conversations.
fn conversation_fingerprint(body: &Value) -> Option<String> {
    let mut hasher = Sha256::new();
    let mut fed = false;
    if let Some(system) = body.get("system") {
        hasher.update(without_cache_control(system).to_string().as_bytes());
        fed = true;
    }
    if let Some(first_user) = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| {
            messages
                .iter()
                .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        })
    {
        hasher.update(without_cache_control(first_user).to_string().as_bytes());
        fed = true;
    }
    if !fed {
        return None;
    }
    let digest = format!("{:x}", hasher.finalize());
    Some(format!("ccsw-{}", &digest[..32]))
}

/// A stable session header the client already sends and the gateway recognises
/// natively (Claude Code, Codex). Checked before deriving an id of our own.
fn native_session_header(headers: &http::HeaderMap) -> Option<&str> {
    [
        "x-claude-code-session-id",
        "session-id",
        "session_id",
        "x-session-id",
    ]
    .iter()
    .find_map(|name| {
        headers
            .get(*name)?
            .to_str()
            .ok()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

/// Pick the value for `x-opencode-session`, most stable identity first: a native
/// session header the client already sends, the client-provided session id, the
/// conversation fingerprint, then the generated id.
fn pick_opencode_session<'a>(
    native: Option<&'a str>,
    client_provided: Option<&'a str>,
    fingerprint: Option<&'a str>,
    generated: &'a str,
) -> Option<String> {
    [native, client_provided, fingerprint, Some(generated)]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

/// Clone `value` without any `cache_control` key, at any depth.
///
/// Clients move that marker between messages as the conversation grows (Claude
/// Code puts it on the first user message, then on the last one), so hashing it
/// would change the session id once, mid-conversation.
fn without_cache_control(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "cache_control")
                .map(|(key, value)| (key.clone(), without_cache_control(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_cache_control).collect()),
        other => other.clone(),
    }
}



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexUpstreamFormat {
    ChatCompletions,
    Anthropic,
}

pub struct ForwardResult {
    pub response: ProxyResponse,
    pub provider: Provider,
    pub claude_api_format: Option<String>,
    /// Wire format selected for this specific Codex Responses request.
    ///
    /// Copilot chooses this per model, so response handlers must consume this
    /// value rather than re-deriving the format from static provider metadata.
    pub codex_upstream_format: Option<CodexUpstreamFormat>,
    /// 实际发往上游的模型名（路由接管/模型映射后的真值）。
    ///
    /// usage 归因不能依赖 ctx.request_model（映射前的客户端别名）：上游响应
    /// 缺失 model 或回显别名时，接管流量会被记成 claude-* 并按其定价计费。
    pub outbound_model: Option<String>,
    /// 活跃连接 RAII guard：随响应一起流转到 response_processor / handle_claude_transform，
    /// 最终被 move 进流式 body future（或非流式响应作用域），覆盖整个响应生命周期。
    pub(crate) connection_guard: Option<ActiveConnectionGuard>,
}

pub struct ForwardError {
    pub error: ProxyError,
    pub provider: Option<Provider>,
}

/// 活跃连接 RAII guard
///
/// 构造时记录请求并把 `ProxyStatus.active_connections` +1；Drop 时优先直接 -1，
/// 锁被占用才调度异步任务，从而支持把 guard move 进流式 body future（stream
/// 自然结束时 guard 与 future 一起 drop）。
///
/// 设计动机：之前在 `forward_with_retry` 出口处同步 -1，但流式响应的 body 实际
/// 在 `create_logged_passthrough_stream` 内还会继续 yield 字节流，导致 UI 的
/// `active_connections` 计数过早归零。RAII guard 让"减量"由 Rust 类型系统驱动，
/// 不需要每条出口路径都手动调用。
pub(crate) struct ActiveConnectionGuard {
    status: Arc<RwLock<ProxyStatus>>,
}

impl ActiveConnectionGuard {
    pub(crate) async fn acquire(status: Arc<RwLock<ProxyStatus>>) -> Self {
        let last_request_at = chrono::Utc::now().to_rfc3339();
        {
            let mut s = status.write().await;
            s.active_connections = s.active_connections.saturating_add(1);
            s.total_requests = s.total_requests.saturating_add(1);
            s.last_request_at = Some(last_request_at);
        }
        Self { status }
    }
}

impl Drop for ActiveConnectionGuard {
    fn drop(&mut self) {
        if let Ok(mut status) = self.status.try_write() {
            status.active_connections = status.active_connections.saturating_sub(1);
            return;
        }
        // Drop 不能 await：把减量操作调度到 tokio runtime
        let status = self.status.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let mut s = status.write().await;
                s.active_connections = s.active_connections.saturating_sub(1);
            });
        }
        // 没有 runtime 时静默丢失计数（仅 UI 展示用，可接受最终一致性）
    }
}

pub struct RequestForwarder {
    /// 共享的 ProviderRouter（持有熔断器状态）
    router: Arc<ProviderRouter>,
    status: Arc<RwLock<ProxyStatus>>,
    current_providers: Arc<RwLock<std::collections::HashMap<String, (String, String)>>>,
    gemini_shadow: Arc<GeminiShadowStore>,
    codex_chat_history: Arc<CodexChatHistoryStore>,
    /// 故障转移切换管理器
    failover_manager: Arc<FailoverSwitchManager>,
    /// AppHandle，用于发射事件和更新托盘
    app_handle: Option<tauri::AppHandle>,
    /// 请求开始时的"当前供应商 ID"（用于判断是否需要同步 UI/托盘）
    current_provider_id_at_start: String,
    /// 代理会话 ID（用于 Gemini Native shadow replay）
    session_id: String,
    /// Session ID 是否由客户端提供；生成值不能作为上游缓存身份。
    session_client_provided: bool,
    /// 整流器配置
    rectifier_config: RectifierConfig,
    /// 优化器配置
    optimizer_config: OptimizerConfig,
    /// 非流式请求超时（秒）
    non_streaming_timeout: std::time::Duration,
    /// 流式请求响应头等待超时（秒）
    streaming_first_byte_timeout: std::time::Duration,
    /// 单个客户端请求最多尝试的 provider 数。
    ///
    /// 由 `AppProxyConfig.max_retries` (UI: "请求失败时的重试次数, 0-10") 派生：
    /// `max_attempts = max_retries + 1`，所以 max_retries=0 表示仅尝试一家、
    /// max_retries=3（默认）表示最多 4 家。loop 同时受 providers.len() 自然限制。
    max_attempts: usize,
}

impl RequestForwarder {
    /// 预防式 media 降级：发送前对 text-only 模型把图片块替换为标记。
    ///
    /// 受 `enabled && request_media_fallback` 管辖；其中"启发式模型名单预测"
    /// 再受 `request_media_heuristic` 单独管辖（显式声明 text-only 始终生效）。
    /// 返回被替换的媒体块数量（0 = 未触发或开关关闭）。
    fn apply_media_prevention(&self, body: &mut Value, provider: &Provider) -> usize {
        if !(self.rectifier_config.enabled && self.rectifier_config.request_media_fallback) {
            return 0;
        }
        let replaced_media = super::media_sanitizer::replace_images_for_text_only_model(
            body,
            provider,
            self.rectifier_config.request_media_heuristic,
        );
        if replaced_media > 0 {
            let model = body.get("model").and_then(Value::as_str).unwrap_or("");
            log::info!(
                "[Media] Replaced {replaced_media} unsupported media block(s) for provider={}, model={}",
                provider.id,
                model
            );
        }
        replaced_media
    }

    /// 反应式 media 重试判定：上游因图片输入报错后，是否应替换图片块并对同一供应商重试一次。
    ///
    /// 受 `enabled && request_media_fallback` 管辖；不涉及 `request_media_heuristic`——
    /// 这里是上游"实测"错误后的纯恢复，不是预测，故启发式开关与它无关。
    fn media_retry_should_trigger(
        &self,
        adapter_name: &str,
        already_retried: bool,
        provider_body: &Value,
        error: &ProxyError,
    ) -> bool {
        matches!(adapter_name, "Claude" | "Codex")
            && self.rectifier_config.enabled
            && self.rectifier_config.request_media_fallback
            && !already_retried
            && super::media_sanitizer::contains_image_blocks(provider_body)
            && super::media_sanitizer::is_unsupported_image_error(error)
    }

    /// 反应式密文重试判定：原样转发的 Responses 上游拒绝了请求里别家签发的密文时，
    /// 返回要去掉哪些状态，对同一供应商重试一次。
    ///
    /// 转成 Chat / Anthropic 的上游收不到这些密文，不参与。受整流器总开关管辖。
    fn opaque_state_retry_rejection(
        &self,
        app_type: &AppType,
        provider: &Provider,
        codex_upstream_format: Option<CodexUpstreamFormat>,
        already_retried: bool,
        request: &Value,
        error: &ProxyError,
    ) -> Option<OpaqueStateRejection> {
        let _ = (app_type, codex_upstream_format, already_retried, request, error, provider);
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        router: Arc<ProviderRouter>,
        non_streaming_timeout: u64,
        status: Arc<RwLock<ProxyStatus>>,
        current_providers: Arc<RwLock<std::collections::HashMap<String, (String, String)>>>,
        gemini_shadow: Arc<GeminiShadowStore>,
        codex_chat_history: Arc<CodexChatHistoryStore>,
        failover_manager: Arc<FailoverSwitchManager>,
        app_handle: Option<tauri::AppHandle>,
        current_provider_id_at_start: String,
        session_id: String,
        session_client_provided: bool,
        streaming_first_byte_timeout: u64,
        _streaming_idle_timeout: u64,
        rectifier_config: RectifierConfig,
        optimizer_config: OptimizerConfig,
        max_retries: u32,
    ) -> Self {
        // max_retries 是「失败后重试次数」语义，attempt 上限 = retries + 1。
        // saturating_add 防止 u32::MAX + 1 溢出。
        let max_attempts = (max_retries as usize).saturating_add(1);
        Self {
            router,
            status,
            current_providers,
            gemini_shadow,
            codex_chat_history,
            failover_manager,
            app_handle,
            current_provider_id_at_start,
            session_id,
            session_client_provided,
            rectifier_config,
            optimizer_config,
            non_streaming_timeout: std::time::Duration::from_secs(non_streaming_timeout),
            streaming_first_byte_timeout: std::time::Duration::from_secs(
                streaming_first_byte_timeout,
            ),
            max_attempts,
        }
    }

    /// 这个请求读写路由状态吗：熔断器（许可、结果、健康度）、「正在使用」、代理统计
    /// （总数、成功率、故障转移次数、最近错误、活跃连接）、故障转移切换。Stack 模型的请求
    /// 和普通路由完全分开，一律不碰；只有用量日志照常按供应商记。以后新增路由状态也先问它。
    fn routing_state_enabled(&self) -> bool {
        // Stack 模式已移除，路由状态一律照记。调用点的 `if` 会在 2c 接线时一并清掉。
        true
    }

    /// 请求体里的模型名已经是这家的上游名（Stack 模型的请求）：不按角色映射、不换成行里配置
    /// 的模型。以后新增模型改写也先问它。
    fn keeps_resolved_model(&self) -> bool {
        // Stack 模式已移除，模型一律按角色映射 / 换成行里配置的模型。
        false
    }

    async fn record_success_result(
        &self,
        provider_id: &str,
        app_type: &str,
        used_half_open_permit: bool,
    ) {
        if used_half_open_permit {
            if let Err(e) = self
                .router
                .record_result(provider_id, app_type, true, true, None)
                .await
            {
                log::warn!(
                    "[{app_type}] 记录 Provider 成功结果失败: provider_id={provider_id}, error={e}"
                );
            }
            return;
        }

        let router = self.router.clone();
        let provider_id = provider_id.to_string();
        let app_type = app_type.to_string();
        tokio::spawn(async move {
            if let Err(e) = router
                .record_result(&provider_id, &app_type, false, true, None)
                .await
            {
                log::warn!(
                    "[{app_type}] 异步记录 Provider 成功结果失败: provider_id={provider_id}, error={e}"
                );
            }
        });
    }

    /// 整流（thinking signature 或 budget）重试失败后的统一收尾。
    ///
    /// `None` 表示已记录熔断器、累积 `last_error`/`last_provider`，
    /// 调用方应 `continue` 让下一家 provider 继续故障转移；
    /// `Some(ForwardError)` 表示是客户端错误，没有 provider 能修复，
    /// 调用方应直接 `return` 把错误返回给客户端。
    #[allow(clippy::too_many_arguments)]
    async fn handle_rectifier_retry_failure(
        &self,
        retry_err: ProxyError,
        provider: &Provider,
        app_type_str: &str,
        used_half_open_permit: bool,
        rectifier_label: &str,
        last_error: &mut Option<ProxyError>,
        last_provider: &mut Option<Provider>,
    ) -> Option<ForwardError> {
        // Provider 错误：本家上游/网络确实出问题，下一家 provider 可能可用 → 继续故障转移。
        // 客户端错误：整流后请求仍违法，下一家也修不好 → 直接返回。
        let is_provider_error = match &retry_err {
            ProxyError::Timeout(_) | ProxyError::ForwardFailed(_) => true,
            ProxyError::UpstreamError { status, .. } => *status >= 500,
            _ => false,
        };

        if is_provider_error {
            self.record_provider_failure(
                provider,
                app_type_str,
                used_half_open_permit,
                &retry_err,
                format!(
                    "Provider {} {rectifier_label}重试失败: {}",
                    provider.name, retry_err
                ),
            )
            .await;
            *last_error = Some(retry_err);
            *last_provider = Some(provider.clone());
            return None;
        }

        Some(
            self.finish_neutral_failure(retry_err, provider, app_type_str, used_half_open_permit)
                .await,
        )
    }

    // 一次尝试和一个客户端请求的收尾记账（熔断器、「正在使用」、代理统计、故障转移切换）
    // 都经下面这几个方法，不在重试循环里直接写。

    /// 开始尝试一家：界面上显示正在尝试哪家。
    async fn note_attempt(&self, provider: &Provider) {
        if !self.routing_state_enabled() {
            return;
        }
        let mut status = self.status.write().await;
        status.current_provider = Some(provider.name.clone());
        status.current_provider_id = Some(provider.id.clone());
    }

    /// 请求成功：记熔断器成功，更新「正在使用」和成功统计。实际用的不是请求开始时那家，
    /// 就计一次故障转移，并异步把当前供应商切过去（同步 UI/托盘）。
    async fn finish_success(
        &self,
        provider: &Provider,
        app_type_str: &str,
        used_half_open_permit: bool,
        (response, claude_api_format, outbound_model): (
            ProxyResponse,
            Option<String>,
            Option<String>,
        ),
        codex_upstream_format: Option<CodexUpstreamFormat>,
    ) -> ForwardResult {
        let result = ForwardResult {
            response,
            provider: provider.clone(),
            claude_api_format,
            codex_upstream_format,
            outbound_model,
            connection_guard: None,
        };
        if !self.routing_state_enabled() {
            return result;
        }

        // 普通闭合熔断状态异步记录，避免阻塞流式首包返回；
        // HalfOpen 探测仍同步等待，保证 permit 与熔断状态及时释放。
        self.record_success_result(&provider.id, app_type_str, used_half_open_permit)
            .await;

        {
            let mut current_providers = self.current_providers.write().await;
            current_providers.insert(
                app_type_str.to_string(),
                (provider.id.clone(), provider.name.clone()),
            );
        }

        {
            let mut status = self.status.write().await;
            status.success_requests += 1;
            status.last_error = None;
            if self.current_provider_id_at_start.as_str() != provider.id.as_str() {
                status.failover_count += 1;

                let fm = self.failover_manager.clone();
                let ah = self.app_handle.clone();
                let pid = provider.id.clone();
                let pname = provider.name.clone();
                let at = app_type_str.to_string();
                tokio::spawn(async move {
                    let _ = fm.try_switch(ah.as_ref(), &at, &pid, &pname).await;
                });
            }
            refresh_success_rate(&mut status);
        }

        result
    }

    /// 这家本身出了问题（下一家可能可用）：记熔断器失败，更新最近错误。调用方接着试下一家。
    async fn record_provider_failure(
        &self,
        provider: &Provider,
        app_type_str: &str,
        used_half_open_permit: bool,
        error: &ProxyError,
        last_error: String,
    ) {
        if !self.routing_state_enabled() {
            return;
        }
        let _ = self
            .router
            .record_result(
                &provider.id,
                app_type_str,
                used_half_open_permit,
                false,
                Some(error.to_string()),
            )
            .await;
        self.status.write().await.last_error = Some(last_error);
    }

    /// 不计入熔断的失败（客户端错误、整流后仍被拒）：只释放 HalfOpen 名额，这个请求记为失败。
    async fn finish_neutral_failure(
        &self,
        error: ProxyError,
        provider: &Provider,
        app_type_str: &str,
        used_half_open_permit: bool,
    ) -> ForwardError {
        if self.routing_state_enabled() {
            self.router
                .release_permit_neutral(&provider.id, app_type_str, used_half_open_permit)
                .await;
            self.record_failed_request(error.to_string()).await;
        }
        ForwardError {
            error,
            provider: Some(provider.clone()),
        }
    }

    /// 这个客户端请求最终失败：失败数加一，记下错误，重算成功率。
    async fn record_failed_request(&self, last_error: String) {
        if !self.routing_state_enabled() {
            return;
        }
        let mut status = self.status.write().await;
        status.failed_requests += 1;
        status.last_error = Some(last_error);
        refresh_success_rate(&mut status);
    }

    /// 转发请求（带故障转移）
    ///
    /// 这是 thin wrapper：在客户端请求维度记一次 `total_requests` / 调整
    /// `active_connections` / 刷新 `last_request_at`，无论 inner 走哪条出口路径，
    /// 出口处都会把 `active_connections` 回收。Per-attempt 维度（成功/失败/熔断
    /// 等）仍由 inner 内自行更新 `success_requests` / `failed_requests`。
    #[allow(clippy::too_many_arguments)]
    pub async fn forward_with_retry(
        &self,
        app_type: &AppType,
        method: http::Method,
        endpoint: &str,
        body: &Value,
        headers: axum::http::HeaderMap,
        extensions: Extensions,
        providers: &[Provider],
    ) -> Result<ForwardResult, ForwardError> {
        if !self.routing_state_enabled() {
            return self
                .forward_with_retry_inner(
                    app_type, method, endpoint, body, headers, extensions, providers,
                )
                .await;
        }
        let guard = ActiveConnectionGuard::acquire(self.status.clone()).await;
        let result = self
            .forward_with_retry_inner(
                app_type, method, endpoint, body, headers, extensions, providers,
            )
            .await;
        // 把 guard 注入到 Ok 结果，让它随响应一起流转到 response_processor，
        // 在流式 body 的 future 内才真正 drop。
        // Err 路径：guard 在函数 scope 内随返回值落地时自动 drop。
        result.map(|mut fr| {
            fr.connection_guard = Some(guard);
            fr
        })
    }

    /// 实际转发逻辑（不包含客户端维度的入口/出口计数）
    ///
    /// # Arguments
    /// * `app_type` - 应用类型
    /// * `method` - 客户端请求的 HTTP 方法（透传给上游，支持 GET/POST 等）
    /// * `endpoint` - API 端点
    /// * `body` - 请求体
    /// * `headers` - 请求头
    /// * `providers` - 已选择的 Provider 列表（由 RequestContext 提供，避免重复调用 select_providers）
    #[allow(clippy::too_many_arguments)]
    async fn forward_with_retry_inner(
        &self,
        app_type: &AppType,
        method: http::Method,
        endpoint: &str,
        body: &Value,
        headers: axum::http::HeaderMap,
        extensions: Extensions,
        providers: &[Provider],
    ) -> Result<ForwardResult, ForwardError> {
        // 获取适配器
        let adapter = get_adapter(app_type).ok_or_else(|| ForwardError {
            error: ProxyError::ConfigError(format!(
                "{} does not support proxy routing",
                app_type.as_str()
            )),
            provider: None,
        })?;
        let app_type_str = app_type.as_str();

        if providers.is_empty() {
            return Err(ForwardError {
                error: ProxyError::NoAvailableProvider,
                provider: None,
            });
        }

        let mut last_error = None;
        let mut last_provider = None;
        let mut attempted_providers = 0usize;

        // 入口协议由客户端请求路径决定；上游协议由激活供应商决定。
        let inbound_protocol = inbound_protocol_for(app_type, endpoint);

        // C′：故障转移链只保留与链头同协议的供应商。
        let failover_chain = same_protocol_failover_chain(app_type, providers);

        // 单 Provider 场景下跳过熔断器检查（故障转移关闭时）；Stack 模型的请求不碰熔断器。
        let bypass_circuit_breaker =
            failover_chain.len() == 1 || !self.routing_state_enabled();

        // 依次尝试每个供应商
        for provider in failover_chain {
            // 整流器重试标记：每个 provider 独立持有，避免标记跨 provider 短路故障转移
            // —— 首家 provider 整流后被 5xx/timeout 击落时，下家仍能用整流后的请求体走整流流程
            let mut rectifier_retried = false;
            let mut budget_rectifier_retried = false;
            let mut media_rectifier_retried = false;
            let mut opaque_rectifier_retried = false;

            // 转换矩阵：入口协议 → 上游协议。没有转换器的格子直接 501，
            // 禁止静默降级成别的协议（矩阵是唯一权威，见 `protocol.rs`）。
            let upstream_protocol = provider_protocol(app_type, provider);
            if matches!(
                Matrix::plan(inbound_protocol, upstream_protocol),
                Conversion::Unsupported
            ) {
                log::warn!(
                    "[{app_type_str}] [FO-007] 入口 {} → 上游 {} 没有转换器，返回 501",
                    inbound_protocol.as_str(),
                    upstream_protocol.as_str()
                );
                return Err(ForwardError {
                    error: ProxyError::ProtocolUnsupported(Matrix::unsupported_message(
                        inbound_protocol,
                        upstream_protocol,
                    )),
                    provider: Some(provider.clone()),
                });
            }

            // 上限检查：尊重用户在 AppProxyConfig.max_retries 上配置的「重试次数」。
            // 放在熔断器 allow 检查之前，避免在已经超限时还占用 HalfOpen 探测名额。
            if attempted_providers >= self.max_attempts {
                log::warn!(
                    "[{app_type_str}] 已达最大尝试次数上限 ({}/{}), 停止故障转移",
                    attempted_providers,
                    self.max_attempts
                );
                break;
            }

            // 发起请求前先获取熔断器放行许可（HalfOpen 会占用探测名额）
            // 单 Provider 场景下跳过此检查，避免熔断器阻塞所有请求
            let (allowed, used_half_open_permit) = if bypass_circuit_breaker {
                (true, false)
            } else {
                let permit = self
                    .router
                    .allow_provider_request(&provider.id, app_type_str)
                    .await;
                (permit.allowed, permit.used_half_open_permit)
            };

            if !allowed {
                continue;
            }

            // PRE-SEND 优化器：每个 provider 独立决定是否优化
            // 只有优化器或错误整流要改写时才复制，避免每家额外复制整段会话历史。
            let mut provider_body = Cow::Borrowed(body);
            if self.optimizer_config.enabled && is_bedrock_provider(provider) {
                if self.optimizer_config.thinking_optimizer {
                    super::thinking_optimizer::optimize(
                        provider_body.to_mut(),
                        &self.optimizer_config,
                    );
                }
                if self.optimizer_config.cache_injection {
                    super::cache_injector::inject(provider_body.to_mut(), &self.optimizer_config);
                }
            }

            attempted_providers += 1;

            // 更新状态中的当前 Provider 信息（per-attempt 维度的标识）
            //
            // total_requests / last_request_at / active_connections 已由
            // forward_with_retry wrapper 在客户端请求维度统一处理，这里只刷
            // 新「正在尝试哪个 provider」的展示字段。
            self.note_attempt(provider).await;

            // 转发请求（每个 Provider 只尝试一次，重试由客户端控制）
            let mut attempted_codex_upstream_format = None;
            match self
                .forward(
                    app_type,
                    &method,
                    provider,
                    endpoint,
                    &provider_body,
                    &headers,
                    &extensions,
                    adapter.as_ref(),
                    &mut attempted_codex_upstream_format,
                )
                .await
            {
                Ok(forwarded) => {
                    return Ok(self
                        .finish_success(
                            provider,
                            app_type_str,
                            used_half_open_permit,
                            forwarded,
                            attempted_codex_upstream_format,
                        )
                        .await);
                }
                Err(mut e) => {
                    // 检测是否需要触发整流器（上游为 Anthropic Messages 的供应商）
                    let is_anthropic_provider = uses_anthropic_rectifiers(
                        ProviderType::from_app_type_and_config(app_type, provider),
                    );
                    let mut signature_rectifier_non_retryable_client_error = false;

                    if self.media_retry_should_trigger(
                        adapter.name(),
                        media_rectifier_retried,
                        &provider_body,
                        &e,
                    ) {
                        let mut media_body = provider_body.as_ref().clone();
                        let replaced_images =
                            super::media_sanitizer::replace_image_blocks_with_marker(
                                &mut media_body,
                            );

                        if replaced_images > 0 {
                            let _ = std::mem::replace(&mut media_rectifier_retried, true);
                            let model = media_body
                                .get("model")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            log::info!(
                                "[{app_type_str}] [Media] Upstream rejected image input; retrying provider={} model={} with {replaced_images} image block(s) replaced by {}",
                                provider.id,
                                model,
                                super::media_sanitizer::UNSUPPORTED_IMAGE_MARKER
                            );

                            let mut media_retry_codex_upstream_format = None;
                            match self
                                .forward(
                                    app_type,
                                    &method,
                                    provider,
                                    endpoint,
                                    &media_body,
                                    &headers,
                                    &extensions,
                                    adapter.as_ref(),
                                    &mut media_retry_codex_upstream_format,
                                )
                                .await
                            {
                                Ok(forwarded) => {
                                    log::info!(
                                        "[{app_type_str}] [Media] Unsupported-image retry succeeded"
                                    );
                                    return Ok(self
                                        .finish_success(
                                            provider,
                                            app_type_str,
                                            used_half_open_permit,
                                            forwarded,
                                            media_retry_codex_upstream_format,
                                        )
                                        .await);
                                }
                                Err(retry_err) => {
                                    log::warn!(
                                        "[{app_type_str}] [Media] Unsupported-image retry still failed: {retry_err}"
                                    );
                                    if let Some(err) = self
                                        .handle_rectifier_retry_failure(
                                            retry_err,
                                            provider,
                                            app_type_str,
                                            used_half_open_permit,
                                            "media 降级",
                                            &mut last_error,
                                            &mut last_provider,
                                        )
                                        .await
                                    {
                                        return Err(err);
                                    }
                                    continue;
                                }
                            }
                        }
                    }

                    if let Some(rejection) = self.opaque_state_retry_rejection(
                        app_type,
                        provider,
                        attempted_codex_upstream_format,
                        opaque_rectifier_retried,
                        &provider_body,
                        &e,
                    ) {
                        // 这一轮是派给子 agent 的加密任务：整流不动它（见
                        // `opaque_state_rectifier`），去掉别家的状态后仍被拒，就是任务本身解不开。
                        let agent_task = carries_encrypted_agent_task(&provider_body);
                        let mut opaque_body = provider_body.as_ref().clone();
                        let rectified = rectify_opaque_state(&mut opaque_body, rejection);
                        if !rectified.applied && agent_task {
                            log::warn!(
                                "[{app_type_str}] [RECT-023] provider={} 解不开子 agent 任务的密文（主、子 agent 不在同一家），不重试",
                                provider.id
                            );
                            return Err(self
                                .finish_neutral_failure(
                                    unreadable_agent_task_error(),
                                    provider,
                                    app_type_str,
                                    used_half_open_permit,
                                )
                                .await);
                        }
                        if rectified.applied {
                            let _ = std::mem::replace(&mut opaque_rectifier_retried, true);
                            log::info!(
                                "[{app_type_str}] [RECT-020] 上游拒绝了请求里别家签发的状态，去掉 {} 个推理条目、{} 个加密片段、{} 个别家 id，换掉 {} 个压缩条目后对 provider={} 重试一次",
                                rectified.removed_reasoning_items,
                                rectified.replaced_encrypted_parts,
                                rectified.removed_foreign_ids,
                                rectified.replaced_compaction_items,
                                provider.id
                            );

                            let mut opaque_retry_codex_upstream_format = None;
                            match self
                                .forward(
                                    app_type,
                                    &method,
                                    provider,
                                    endpoint,
                                    &opaque_body,
                                    &headers,
                                    &extensions,
                                    adapter.as_ref(),
                                    &mut opaque_retry_codex_upstream_format,
                                )
                                .await
                            {
                                Ok(forwarded) => {
                                    log::info!("[{app_type_str}] [RECT-021] 密文整流重试成功");
                                    return Ok(self
                                        .finish_success(
                                            provider,
                                            app_type_str,
                                            used_half_open_permit,
                                            forwarded,
                                            opaque_retry_codex_upstream_format,
                                        )
                                        .await);
                                }
                                // 重试仍失败：按这次的错误走常规分类，和没整流过一样
                                // 决定是否计入熔断、是否换下一家。
                                Err(retry_err) => {
                                    log::warn!(
                                        "[{app_type_str}] [RECT-022] 密文整流重试仍失败: {retry_err}"
                                    );
                                    if agent_task
                                        && self
                                            .opaque_state_retry_rejection(
                                                app_type,
                                                provider,
                                                opaque_retry_codex_upstream_format,
                                                false,
                                                &opaque_body,
                                                &retry_err,
                                            )
                                            .is_some()
                                    {
                                        log::warn!(
                                            "[{app_type_str}] [RECT-023] provider={} 解不开子 agent 任务的密文（主、子 agent 不在同一家），不再重试",
                                            provider.id
                                        );
                                        return Err(self
                                            .finish_neutral_failure(
                                                unreadable_agent_task_error(),
                                                provider,
                                                app_type_str,
                                                used_half_open_permit,
                                            )
                                            .await);
                                    }
                                    e = retry_err;
                                }
                            }
                        }
                    }

                    if is_anthropic_provider {
                        let error_message = extract_error_message(&e);
                        if should_rectify_thinking_signature(
                            error_message.as_deref(),
                            &self.rectifier_config,
                        ) {
                            // 已经重试过：直接返回错误（不可重试客户端错误）
                            if rectifier_retried {
                                log::warn!("[{app_type_str}] [RECT-005] 整流器已触发过，不再重试");
                                // 不记录熔断器，这是客户端兼容性问题
                                return Err(self
                                    .finish_neutral_failure(
                                        e,
                                        provider,
                                        app_type_str,
                                        used_half_open_permit,
                                    )
                                    .await);
                            }

                            // 首次触发：整流请求体
                            let rectified = rectify_anthropic_request(provider_body.to_mut());

                            // 整流未生效：继续尝试 budget 整流路径，避免误判后短路
                            if !rectified.applied {
                                log::warn!(
                                    "[{app_type_str}] [RECT-006] thinking 签名整流器触发但无可整流内容，继续检查 budget；若 budget 也未命中则按客户端错误返回"
                                );
                                signature_rectifier_non_retryable_client_error = true;
                            } else {
                                log::info!(
                                    "[{}] [RECT-001] thinking 签名整流器触发, 移除 {} thinking blocks, {} redacted_thinking blocks, {} signature fields",
                                    app_type_str,
                                    rectified.removed_thinking_blocks,
                                    rectified.removed_redacted_thinking_blocks,
                                    rectified.removed_signature_fields
                                );

                                // 标记已重试（当前逻辑下重试后必定 return，保留标记以备将来扩展）
                                let _ = std::mem::replace(&mut rectifier_retried, true);

                                // 使用同一供应商重试（不计入熔断器）
                                let mut signature_retry_codex_upstream_format = None;
                                match self
                                    .forward(
                                        app_type,
                                        &method,
                                        provider,
                                        endpoint,
                                        &provider_body,
                                        &headers,
                                        &extensions,
                                        adapter.as_ref(),
                                        &mut signature_retry_codex_upstream_format,
                                    )
                                    .await
                                {
                                    Ok(forwarded) => {
                                        log::info!("[{app_type_str}] [RECT-002] 整流重试成功");
                                        return Ok(self
                                            .finish_success(
                                                provider,
                                                app_type_str,
                                                used_half_open_permit,
                                                forwarded,
                                                signature_retry_codex_upstream_format,
                                            )
                                            .await);
                                    }
                                    Err(retry_err) => {
                                        log::warn!(
                                            "[{app_type_str}] [RECT-003] 整流重试仍失败: {retry_err}"
                                        );
                                        if let Some(err) = self
                                            .handle_rectifier_retry_failure(
                                                retry_err,
                                                provider,
                                                app_type_str,
                                                used_half_open_permit,
                                                "整流",
                                                &mut last_error,
                                                &mut last_provider,
                                            )
                                            .await
                                        {
                                            return Err(err);
                                        }
                                        continue;
                                    }
                                }
                            }
                        }
                    }

                    // 检测是否需要触发 budget 整流器（上游为 Anthropic Messages 的供应商）
                    if is_anthropic_provider {
                        let error_message = extract_error_message(&e);
                        if should_rectify_thinking_budget(
                            error_message.as_deref(),
                            &self.rectifier_config,
                        ) {
                            // 已经重试过：直接返回错误（不可重试客户端错误）
                            if budget_rectifier_retried {
                                log::warn!(
                                    "[{app_type_str}] [RECT-013] budget 整流器已触发过，不再重试"
                                );
                                return Err(self
                                    .finish_neutral_failure(
                                        e,
                                        provider,
                                        app_type_str,
                                        used_half_open_permit,
                                    )
                                    .await);
                            }

                            let budget_rectified = rectify_thinking_budget(provider_body.to_mut());
                            if !budget_rectified.applied {
                                log::warn!(
                                    "[{app_type_str}] [RECT-014] budget 整流器触发但无可整流内容，不做无意义重试"
                                );
                                return Err(self
                                    .finish_neutral_failure(
                                        e,
                                        provider,
                                        app_type_str,
                                        used_half_open_permit,
                                    )
                                    .await);
                            }

                            log::info!(
                                "[{}] [RECT-010] thinking budget 整流器触发, before={:?}, after={:?}",
                                app_type_str,
                                budget_rectified.before,
                                budget_rectified.after
                            );

                            let _ = std::mem::replace(&mut budget_rectifier_retried, true);

                            // 使用同一供应商重试（不计入熔断器）
                            let mut budget_retry_codex_upstream_format = None;
                            match self
                                .forward(
                                    app_type,
                                    &method,
                                    provider,
                                    endpoint,
                                    &provider_body,
                                    &headers,
                                    &extensions,
                                    adapter.as_ref(),
                                    &mut budget_retry_codex_upstream_format,
                                )
                                .await
                            {
                                Ok(forwarded) => {
                                    log::info!("[{app_type_str}] [RECT-011] budget 整流重试成功");
                                    return Ok(self
                                        .finish_success(
                                            provider,
                                            app_type_str,
                                            used_half_open_permit,
                                            forwarded,
                                            budget_retry_codex_upstream_format,
                                        )
                                        .await);
                                }
                                Err(retry_err) => {
                                    log::warn!(
                                        "[{app_type_str}] [RECT-012] budget 整流重试仍失败: {retry_err}"
                                    );
                                    if let Some(err) = self
                                        .handle_rectifier_retry_failure(
                                            retry_err,
                                            provider,
                                            app_type_str,
                                            used_half_open_permit,
                                            "budget 整流",
                                            &mut last_error,
                                            &mut last_provider,
                                        )
                                        .await
                                    {
                                        return Err(err);
                                    }
                                    continue;
                                }
                            }
                        }
                    }

                    if signature_rectifier_non_retryable_client_error {
                        return Err(self
                            .finish_neutral_failure(
                                e,
                                provider,
                                app_type_str,
                                used_half_open_permit,
                            )
                            .await);
                    }

                    // 先分类错误，决定是否计入 provider 健康度
                    // —— NonRetryable / ClientAbort 是客户端层错误，无论换哪家 provider 都会被拒绝，
                    //    不应污染熔断器和数据库健康度（与 release_permit_neutral 同语义）。
                    let category = self.categorize_proxy_error(&e, provider);

                    match category {
                        ErrorCategory::Retryable => {
                            // 可重试：真正的 provider 故障 → 记录失败并更新熔断器/DB 健康度
                            self.record_provider_failure(
                                provider,
                                app_type_str,
                                used_half_open_permit,
                                &e,
                                format!("Provider {} 失败: {}", provider.name, e),
                            )
                            .await;

                            let (log_code, log_message) = build_retryable_failure_log(
                                &provider.name,
                                attempted_providers,
                                providers.len(),
                                &e,
                            );
                            log::warn!("[{app_type_str}] [{log_code}] {log_message}");

                            last_error = Some(e);
                            last_provider = Some(provider.clone());
                            // 继续尝试下一个供应商
                            continue;
                        }
                        ErrorCategory::NonRetryable | ErrorCategory::ClientAbort => {
                            // 不可重试：客户端层错误或客户端断连 → 不污染健康度，仅释放 HalfOpen permit
                            return Err(self
                                .finish_neutral_failure(
                                    e,
                                    provider,
                                    app_type_str,
                                    used_half_open_permit,
                                )
                                .await);
                        }
                    }
                }
            }
        }

        if attempted_providers == 0 {
            // providers 列表非空，但全部被熔断器拒绝（典型：HalfOpen 探测名额被占用）
            self.record_failed_request("所有供应商暂时不可用（熔断器限制）".to_string())
                .await;
            return Err(ForwardError {
                error: ProxyError::NoAvailableProvider,
                provider: None,
            });
        }

        // 所有供应商都失败了
        self.record_failed_request("所有供应商都失败".to_string())
            .await;

        if let Some((log_code, log_message)) =
            build_terminal_failure_log(attempted_providers, providers.len(), last_error.as_ref())
        {
            log::warn!("[{app_type_str}] [{log_code}] {log_message}");
        }

        Err(ForwardError {
            error: last_error.unwrap_or(ProxyError::MaxRetriesExceeded),
            provider: last_provider,
        })
    }

    /// 转发单个请求（使用适配器）
    ///
    /// 成功时返回 `(response, claude_api_format, outbound_model)`，其中
    /// `outbound_model` 是最终发往上游的模型名（所有映射/改写之后）。
    /// `codex_upstream_format_out` 记下这次实际选中的 Codex 上游格式：成功时交给
    /// `finish_success`，失败时给整流判断用（Copilot 按模型逐次选协议，不能从静态配置推）。
    #[allow(clippy::too_many_arguments)]
    async fn forward(
        &self,
        app_type: &AppType,
        method: &http::Method,
        provider: &Provider,
        endpoint: &str,
        body: &Value,
        headers: &axum::http::HeaderMap,
        extensions: &Extensions,
        adapter: &dyn ProviderAdapter,
        codex_upstream_format_out: &mut Option<CodexUpstreamFormat>,
    ) -> Result<(ProxyResponse, Option<String>, Option<String>), ProxyError> {
        *codex_upstream_format_out = None;

        // P3：Gemini 的模型名在 URI 里，默认模型必须改路径而不是 body。
        // 其余三种协议在下面的 `apply_model_mapping` 里统一改 body。
        let endpoint = endpoint;

        // 使用适配器提取 base_url
        let base_url = adapter.extract_base_url(provider)?;

        let is_full_url = provider
            .meta
            .as_ref()
            .and_then(|meta| meta.is_full_url)
            .unwrap_or(false);

        // 唯一的 API 入口不再支持 Copilot / Codex 客户端形态。
        let is_copilot = false;

        // Responses 入口 × Chat / Anthropic 上游：请求方向改 endpoint 与 body，
        // 响应方向由 handle_responses 依据 `codex_upstream_format` 还原成 Responses 形状。
        // 判定只看协议矩阵，不再依赖客户端家族。
        let inbound_protocol = inbound_protocol_for(app_type, endpoint);
        let upstream_protocol = provider_protocol(app_type, provider);
        let codex_responses_to_chat = inbound_protocol == Protocol::OpenAiResponses
            && upstream_protocol == Protocol::OpenAiChat;
        let codex_responses_to_anthropic = inbound_protocol == Protocol::OpenAiResponses
            && upstream_protocol == Protocol::Anthropic;
        // P4：Chat 入口 × Responses 上游。请求方向改 endpoint 与 body，
        // 响应方向由 handle_chat_completions 依据供应商协议转回 Chat 形状。
        let chat_to_responses = !codex_responses_to_chat
            && !codex_responses_to_anthropic
            && inbound_protocol == Protocol::OpenAiChat
            && upstream_protocol == Protocol::OpenAiResponses;
        // Chat 入口 × Anthropic 上游：直连转换，响应方向由
        // handle_chat_completions 转回 Chat 形状。
        let chat_to_anthropic = inbound_protocol == Protocol::OpenAiChat
            && upstream_protocol == Protocol::Anthropic;
        // 模型名直接透传，不再做任何 Claude Code 模型映射 / 1M 后缀处理。
        let mapped_body = body.clone();

        // 与 CCH 对齐：请求前不做 thinking 主动改写（仅保留兼容入口）
        let mut mapped_body = normalize_thinking_type(mapped_body);
        let is_copilot_codex_responses = false;

        // The OpenCode gateway wants a stable per-conversation session id (see the
        // injection site below). It has to be derived here: `mapped_body` is moved
        // by the format conversion further down.
        //
        // Only that upstream consumes it, and only when the client neither sent
        // `x-opencode-session` nor supplied a session id, so guard the work: the
        // digest serialises the system prompt plus the first user message, which
        // routinely carry files and images and can reach megabytes. The host check
        // uses `base_url` - none of the URL-construction branches below rewrites the
        // authority (the only one that replaces the host wholesale is Copilot's
        // dynamic endpoint, which points at GitHub, never opencode.ai).
        let base_url_host = base_url
            .parse::<http::Uri>()
            .ok()
            .and_then(|u| u.authority().map(|a| a.to_string()));
        let opencode_conversation_fingerprint = if !self.session_client_provided
            && !headers.contains_key("x-opencode-session")
            && native_session_header(headers).is_none()
            && is_opencode_upstream(base_url_host.as_deref())
        {
            conversation_fingerprint(&mapped_body)
        } else {
            None
        };

        *codex_upstream_format_out = if codex_responses_to_anthropic {
            Some(CodexUpstreamFormat::Anthropic)
        } else if codex_responses_to_chat {
            Some(CodexUpstreamFormat::ChatCompletions)
        } else {
            None
        };
        let resolved_claude_api_format = if adapter.name() == "Claude" {
            Some(
                self.resolve_claude_api_format(provider, &mapped_body)
                    .await,
            )
        } else {
            None
        };
        // Claude 的请求体规范化/转换只适用于 Anthropic 入口。Responses / Chat
        // 入口有自己的转换分支（上面三个 flag），不能被 Claude 转换器再加工。
        let claude_inbound = inbound_protocol == Protocol::Anthropic;
        if adapter.name() == "Claude" && claude_inbound {
            if let Some(api_format) = resolved_claude_api_format.as_deref() {
                super::providers::normalize_anthropic_messages_for_provider(
                    &mut mapped_body,
                    provider,
                    api_format,
                );
                self.apply_media_prevention(&mut mapped_body, provider);
            }
        }
        let needs_transform = claude_inbound
            && match resolved_claude_api_format.as_deref() {
                Some(api_format) => super::providers::claude_api_format_needs_transform(api_format),
                None => adapter.needs_transform(provider),
            };
        // Codex → Anthropic: Claude Code emulation is off by default and only
        // enabled when the user explicitly turns it on in the UI, so requests can
        // pass a gateway's "Claude Code only" fingerprint check (User-Agent /
        // anthropic-beta / x-app / system prompt first line). Defaulting to off
        // avoids leaking the Claude Code fingerprint and identity prompt to
        // general-purpose gateways.
        let codex_impersonate_claude_code = (codex_responses_to_anthropic || chat_to_anthropic)
            && provider
                .meta
                .as_ref()
                .and_then(|meta| meta.impersonate_claude_code)
                == Some(true);
        let (effective_endpoint, passthrough_query) =
            if codex_responses_to_chat {
                rewrite_codex_responses_endpoint_to_chat(endpoint)
            } else if codex_responses_to_anthropic {
                rewrite_codex_responses_endpoint_to_anthropic(endpoint)
            } else if chat_to_responses {
                rewrite_chat_endpoint_to_responses(endpoint)
            } else if chat_to_anthropic {
                rewrite_chat_endpoint_to_anthropic(endpoint)
            } else if needs_transform && adapter.name() == "Claude" {
                let api_format = resolved_claude_api_format
                    .as_deref()
                    .unwrap_or_else(|| super::providers::get_claude_api_format(provider));
                rewrite_claude_transform_endpoint(endpoint, api_format, false, &mapped_body)
            } else {
                (
                    endpoint.to_string(),
                    split_endpoint_and_query(endpoint)
                        .1
                        .map(ToString::to_string),
                )
            };

        let codex_chat_base_is_full_endpoint =
            codex_responses_to_chat && base_url_is_full_endpoint(&base_url, "/chat/completions");

        // Defensive fallback mirroring `codex_chat_base_is_full_endpoint`: if a user pastes
        // a base URL already ending in the Anthropic `/v1/messages` endpoint but leaves the
        // "full URL" switch off, treat it as a full endpoint so we don't double-append
        // `/v1/messages` (→ `.../v1/messages/v1/messages`, a non-retryable 400). Matches the
        // exact endpoint suffix, so prefixed gateways like `.../api/v1/messages` are covered.
        let codex_anthropic_base_is_full_endpoint =
            codex_responses_to_anthropic && base_url_is_full_endpoint(&base_url, "/v1/messages");
        let chat_responses_base_is_full_endpoint =
            chat_to_responses && base_url_is_full_endpoint(&base_url, "/responses");
        let chat_responses_base_is_chat_endpoint =
            chat_to_responses && base_url_is_full_endpoint(&base_url, "/chat/completions");

        let url = if matches!(resolved_claude_api_format.as_deref(), Some("gemini_native")) {
            super::gemini_url::resolve_gemini_native_url(
                &base_url,
                &effective_endpoint,
                is_full_url,
            )
        } else if is_full_url {
            if chat_to_responses {
                let rewritten = rewrite_base_url_chat_to_responses(&base_url);
                append_query_to_full_url(&rewritten, passthrough_query.as_deref())
            } else {
                append_query_to_full_url(&base_url, passthrough_query.as_deref())
            }
        } else if chat_responses_base_is_chat_endpoint {
            let rewritten = rewrite_base_url_chat_to_responses(&base_url);
            append_query_to_full_url(&rewritten, passthrough_query.as_deref())
        } else if chat_responses_base_is_full_endpoint {
            append_query_to_full_url(&base_url, passthrough_query.as_deref())
        } else if codex_chat_base_is_full_endpoint || codex_anthropic_base_is_full_endpoint {
            append_query_to_full_url(&base_url, passthrough_query.as_deref())
        } else {
            adapter.build_url(&base_url, &effective_endpoint)
        };

        let (listen_address, listen_port) = {
            let status = self.status.read().await;
            (status.address.clone(), status.port)
        };
        super::upstream_guard::reject_self_forwarding(&url, &listen_address, listen_port).await?;

        // 记录映射后的出站模型名（此时 mapped_body 已完成接管映射 / [1m] 剥离 /
        // Copilot 归一化）。格式转换后若 body 仍带 model 字段会在下方刷新覆盖；
        // gemini_native 等模型在 URL 中的格式则保留此处的转换前真值。
        let mut outbound_model = mapped_body
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
            .map(str::to_string);

        // 转换请求体（如果需要）
        let mut request_body = if codex_responses_to_chat {
            let mut mapped_body = mapped_body;
            let explicit_prompt_cache_key = mapped_body
                .get("prompt_cache_key")
                .and_then(|value| value.as_str())
                .map(ToString::to_string);
            let restored = self
                .codex_chat_history
                .enrich_request(&mut mapped_body)
                .await;
            if restored > 0 {
                log::debug!(
                    "[Codex] Restored or enriched {restored} cached function call item(s) for Chat upstream"
                );
            }
            if !self.keeps_resolved_model() && !is_copilot_codex_responses {
                super::providers::apply_codex_chat_upstream_model(provider, &mut mapped_body);
            }
            let reasoning_config =
                super::providers::resolve_codex_chat_reasoning_config(provider, &mapped_body);
            let mut chat_body = super::providers::transform_codex_chat::responses_to_chat_completions_with_reasoning(
                mapped_body,
                reasoning_config.as_ref(),
            )?;
            let signature_fallbacks =
                super::providers::codex_chat_common::fill_gemini_thought_signature_fallback(
                    &mut chat_body,
                );
            if signature_fallbacks > 0 {
                log::debug!(
                    "[Codex] Filled {signature_fallbacks} missing Gemini thought signature(s) with the skip placeholder"
                );
            }
            super::providers::inject_codex_chat_prompt_cache_key(
                provider,
                &mut chat_body,
                explicit_prompt_cache_key.as_deref(),
                self.session_client_provided
                    .then_some(self.session_id.as_str()),
            );
            chat_body
        } else if codex_responses_to_anthropic {
            let mut mapped_body = mapped_body;
            if !self.keeps_resolved_model() {
                super::providers::apply_codex_upstream_model(provider, &mut mapped_body);
            }
            // Per-provider output ceiling override. Codex does not forward its
            // `model_max_output_tokens` in the request body, so honor the value
            // configured on the provider here — it takes precedence over any
            // request-supplied `max_output_tokens` and over the default below.
            // Injecting it into the body (rather than overriding after transform)
            // lets the thinking-budget clamp size its headroom against the real
            // ceiling too. Kept per-provider to avoid a global large default that
            // would 400 on low-output-ceiling gateways.
            if let Some(max_out) = provider
                .meta
                .as_ref()
                .and_then(|meta| meta.max_output_tokens)
                .filter(|v| *v > 0)
            {
                mapped_body["max_output_tokens"] = Value::from(max_out);
            }
            // Anthropic requires max_tokens; fall back to this default only when the
            // Codex request omits max_output_tokens (rare — Codex normally sends it).
            // Kept conservative so a low-output-ceiling model or relay does not hard-400
            // on the fallback (a too-high default 400s and is non-retryable); 8192 is
            // accepted by every current Claude model and virtually all gateways. The
            // transform clamps any thinking budget below this value.
            const DEFAULT_CODEX_ANTHROPIC_MAX_TOKENS: u64 = 8192;
            let mut anthropic_body =
                super::providers::transform_codex_anthropic::responses_request_to_anthropic(
                    mapped_body,
                    DEFAULT_CODEX_ANTHROPIC_MAX_TOKENS,
                )?;
            if codex_impersonate_claude_code {
                prepend_claude_code_system_prompt(&mut anthropic_body);
            }
            // Enable Anthropic prompt caching (no beta header required). Reuse the
            // configured TTL rather than silently forcing 5m on this conversion path.
            // otherwise system/tools/history are re-sent at full price every round,
            // inflating cost and first-token latency. The injector handles the
            // string→array `system` conversion and the new-breakpoint budget.
            super::cache_injector::inject(
                &mut anthropic_body,
                &codex_anthropic_cache_config(&self.optimizer_config),
            );
            anthropic_body
        } else if chat_to_responses {
            super::providers::transform_chat_responses::chat_request_to_responses(mapped_body)?
        } else if chat_to_anthropic {
            super::providers::transform_chat_anthropic::chat_request_to_anthropic(mapped_body)?
        } else if needs_transform {
            if adapter.name() == "Claude" {
                let api_format = resolved_claude_api_format
                    .as_deref()
                    .unwrap_or_else(|| super::providers::get_claude_api_format(provider));
                let mut mapped_body = mapped_body;
                // 映射后的模型名不再带「省略 thinking 仍默认开思考」的语义（5.x 的
                // Fable / Mythos / Opus / Sonnet），按客户端原模型补成显式 adaptive，
                // effort 换算才不会当成关闭。
                if matches!(
                    api_format,
                    "openai_chat" | "openai_responses" | "gemini_native"
                ) {
                    super::providers::transform::make_default_adaptive_thinking_explicit(
                        &mut mapped_body,
                        body.get("model").and_then(|m| m.as_str()),
                    );
                }
                super::providers::transform_claude_request_for_api_format(
                    mapped_body,
                    provider,
                    api_format,
                    self.session_client_provided
                        .then_some(self.session_id.as_str()),
                    Some(self.gemini_shadow.as_ref()),
                )?
            } else {
                adapter.transform_request(mapped_body, provider)?
            }
        } else {
            mapped_body
        };

        // Codex 远程压缩，以及同一线程里别家回合留下的状态（见 `codex_compaction`）。
        // Chat / Anthropic 转换在转换器里处理；这里管原样转发的两种上游：官方清掉
        // CC Switch 产出、官方一定会拒的条目，原生 Responses 第三方把压缩触发和
        // CC Switch 包装的压缩摘要改成普通消息。看不出来源的密文原样发出，被拒后由
        // `opaque_state_rectifier` 处理。请求里没有这些条目时逐字节不变。
        if false {
            if false {
                if super::providers::codex_compaction::scrub_ccswitch_state_for_official(
                    &mut request_body,
                ) {
                    log::debug!(
                        "[Codex] Scrubbed third-party turn state before the official upstream (provider={})",
                        provider.id
                    );
                }
            } else if super::providers::codex_compaction::prepare_native_third_party_request(
                &mut request_body,
            ) {
                log::debug!(
                    "[Codex] Rewrote compaction items for a native Responses upstream (provider={})",
                    provider.id
                );
            }
        }

        // Native Responses passthrough to a strict third-party gateway (xAI).
        // One gate so rebase conflicts stay here plus the isolate file, not
        // scattered across sanitizers. Flatten namespaces first; then apply
        // xAI request rewrites (schema, agent_message, unknown models).
        if false {
            if super::providers::transform_codex_responses_namespace::flatten_request_namespaces(
                &mut request_body,
            )? {
                log::debug!(
                    "[Codex] Flattened namespace tools for native Responses upstream (provider={})",
                    provider.id
                );
            }
            // Stack 请求只做字段兼容。
            let upstream_model = if self.keeps_resolved_model() {
                None
            } else {
                super::providers::codex_provider_upstream_model(provider)
            };
            super::providers::transform_codex_responses_xai_sanitize::apply_xai_native_responses_request_compat(
                &mut request_body,
                &provider.id,
                upstream_model.as_deref(),
                &provider.settings_config,
            );
        }

        // Moonshot / Kimi Chat Completions reject `$ref` nodes that carry sibling
        // keywords, which Codex Desktop's built-in tool schemas do (#6867). Move
        // each such `$ref` into `allOf` for that upstream only; every other
        // provider keeps byte-identical tool schemas (prompt-cache prefix intact).
        if codex_responses_to_chat
            && super::providers::transform_codex_chat_moonshot_schema::upstream_requires_ref_sibling_all_of(
                &base_url,
            )
        {
            let rewritten =
                super::providers::transform_codex_chat_moonshot_schema::wrap_ref_siblings_in_chat_tools(
                    &mut request_body,
                );
            if rewritten > 0 {
                log::debug!(
                    "[Codex] Moved `$ref` siblings into allOf for {rewritten} tool schema(s) (Moonshot upstream, provider={})",
                    provider.id
                );
            }
        }

        if false {
            self.apply_media_prevention(&mut request_body, provider);
        }

        // 过滤私有参数（以 `_` 开头的字段），防止内部信息泄露到上游
        // 默认使用空白名单，过滤所有 _ 前缀字段
        let mut filtered_body = prepare_upstream_request_body(request_body);
        if !is_copilot {
            if let Some(overrides) = provider
                .meta
                .as_ref()
                .and_then(|meta| meta.local_proxy_request_overrides.as_ref())
            {
                if apply_local_proxy_body_overrides(&mut filtered_body, overrides) {
                    filtered_body = prepare_upstream_request_body(filtered_body);
                }
            }
        }
        // 出站 body 定稿后刷新真值（覆盖 Codex chat 上游模型覆写、转换层模型改写）
        if let Some(m) = filtered_body
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
        {
            outbound_model = Some(m.to_string());
        }
        log_prompt_cache_trace(
            app_type,
            provider,
            &effective_endpoint,
            resolved_claude_api_format.as_deref(),
            &filtered_body,
            self.session_client_provided,
        );
        let request_is_streaming =
            is_streaming_request(&effective_endpoint, &filtered_body, headers);
        let force_identity_encoding = needs_transform
            || codex_responses_to_chat
            || codex_responses_to_anthropic
            || chat_to_anthropic
            || request_is_streaming;

        // 获取认证头（提前准备，用于内联替换），同时保留仅用于日志脱敏的
        // 精确认证材料。实际日志永远不输出这些值。
        let mut log_secrets: Vec<String> = Vec::new();
        let mut auth_headers = if let Some(auth) = adapter.extract_auth(provider) {
            for secret in std::iter::once(&auth.api_key).chain(auth.access_token.iter()) {
                if !secret.is_empty() && !log_secrets.contains(secret) {
                    log_secrets.push(secret.clone());
                }
            }

            adapter.get_auth_headers(&auth)?
        } else {
            Vec::new()
        };

        // 自定义 User-Agent：与 stream_check / model_fetch 共用 parse_custom_user_agent，
        // 运行时静默忽略非法值（前端在输入处给非阻断提示，不在保存时阻断）。
        // Copilot 指纹 UA 不可覆盖。
        let custom_user_agent = if is_copilot {
            None
        } else {
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.custom_user_agent_header().ok().flatten())
        };
        // Codex→Anthropic emulation: when there is no custom UA, override Codex's
        // codex_cli_rs UA with the Claude Code UA.
        let custom_user_agent = if custom_user_agent.is_none() && codex_impersonate_claude_code {
            Some(http::HeaderValue::from_static(CLAUDE_CODE_USER_AGENT))
        } else {
            custom_user_agent
        };

        // 已移除 Copilot 指纹头。
        let copilot_fingerprint_headers: &[&str] = &[];

        // 预计算上游 host 值（用于在原位替换 host header）
        let upstream_host = url
            .parse::<http::Uri>()
            .ok()
            .and_then(|u| u.authority().map(|a| a.to_string()));

        // OpenCode Go places two hard requirements on clients
        // (<https://opencode.ai/docs/go/>); satisfy both, only for its upstream.
        //
        // 1) User-Agent: a client that identifies itself is passed through
        //    untouched (Claude Code, Codex, ...). Only a request that carries no
        //    UA at all gets this fallback - the gateway sits behind Cloudflare and
        //    rejects unidentified clients (403 Access denied; a UA clears it).
        //    The fallback is therefore applied after the header loop, next to the
        //    custom UA. An explicitly configured custom UA still wins.
        let opencode_fallback_user_agent =
            if custom_user_agent.is_none() && is_opencode_upstream(upstream_host.as_deref()) {
                Some(http::HeaderValue::from_static(concat!(
                    "cc-switch/",
                    env!("CARGO_PKG_VERSION")
                )))
            } else {
                None
            };

        // 2) Session header: without `x-opencode-session` the gateway answers
        //    400 MissingSessionID. Pass it through when the client already sends
        //    one, otherwise send the most stable identity we have
        //    (`pick_opencode_session`) - a native client header, the client's
        //    session id, the conversation fingerprint, or the generated id.
        if is_opencode_upstream(upstream_host.as_deref())
            && !headers.contains_key("x-opencode-session")
        {
            let client_id = self
                .session_client_provided
                .then_some(self.session_id.as_str());
            if let Some(session) = pick_opencode_session(
                native_session_header(headers),
                client_id,
                opencode_conversation_fingerprint.as_deref(),
                &self.session_id,
            ) {
                if let Ok(value) = http::HeaderValue::from_str(&session) {
                    auth_headers.push((http::HeaderName::from_static("x-opencode-session"), value));
                }
            }
        }

        let should_send_anthropic_headers = adapter.name() == "Claude"
            && matches!(resolved_claude_api_format.as_deref(), Some("anthropic"));

        // 预计算 anthropic-beta 值（仅 Claude）
        let anthropic_beta_value = if should_send_anthropic_headers {
            const CLAUDE_CODE_BETA: &str = "claude-code-20250219";
            Some(if let Some(beta) = headers.get("anthropic-beta") {
                if let Ok(beta_str) = beta.to_str() {
                    if beta_str.contains(CLAUDE_CODE_BETA) {
                        beta_str.to_string()
                    } else {
                        format!("{CLAUDE_CODE_BETA},{beta_str}")
                    }
                } else {
                    CLAUDE_CODE_BETA.to_string()
                }
            } else {
                CLAUDE_CODE_BETA.to_string()
            })
        } else if codex_impersonate_claude_code {
            // Codex→Anthropic: 模拟 Claude Code 客户端标记。
            Some("claude-code-20250219".to_string())
        } else {
            None
        };

        // ============================================================
        // 构建有序 HeaderMap — 内联替换，保持客户端原始顺序
        // ============================================================
        let mut ordered_headers = http::HeaderMap::new();
        let mut saw_auth = false;
        let mut saw_accept_encoding = false;
        let mut saw_accept = false;
        let mut saw_user_agent = false;
        let mut saw_anthropic_beta = false;
        let mut saw_anthropic_version = false;

        for (key, value) in headers {
            let key_str = key.as_str();

            // --- host — 原位替换为上游 host（保持客户端原始位置） ---
            if key_str.eq_ignore_ascii_case("host") {
                if let Some(ref host_val) = upstream_host {
                    if let Ok(hv) = http::HeaderValue::from_str(host_val) {
                        ordered_headers.append(key.clone(), hv);
                    }
                }
                continue;
            }

            // --- 连接 / 追踪 / CDN 类 — 无条件跳过 ---
            if matches!(
                key_str,
                "content-length"
                    | "transfer-encoding"
                    | "x-forwarded-host"
                    | "x-forwarded-port"
                    | "x-forwarded-proto"
                    | "forwarded"
                    | "cf-connecting-ip"
                    | "cf-ipcountry"
                    | "cf-ray"
                    | "cf-visitor"
                    | "true-client-ip"
                    | "fastly-client-ip"
                    | "x-azure-clientip"
                    | "x-azure-fdid"
                    | "x-azure-ref"
                    | "akamai-origin-hop"
                    | "x-akamai-config-log-detail"
                    | "x-request-id"
                    | "x-correlation-id"
                    | "x-trace-id"
                    | "x-amzn-trace-id"
                    | "x-b3-traceid"
                    | "x-b3-spanid"
                    | "x-b3-parentspanid"
                    | "x-b3-sampled"
                    | "traceparent"
                    | "tracestate"
            ) {
                continue;
            }

            // --- 认证类 — 用 adapter 提供的认证头替换（在原始位置） ---
            if key_str.eq_ignore_ascii_case("authorization")
                || key_str.eq_ignore_ascii_case("x-api-key")
                || key_str.eq_ignore_ascii_case("x-goog-api-key")
            {
                if !saw_auth {
                    saw_auth = true;
                    for (ah_name, ah_value) in &auth_headers {
                        ordered_headers.append(ah_name.clone(), ah_value.clone());
                    }
                }
                continue;
            }

            // --- x-app — during Codex→Anthropic emulation, `cli` is injected uniformly below ---
            if codex_impersonate_claude_code && key_str.eq_ignore_ascii_case("x-app") {
                continue;
            }

            // --- Codex/OpenAI fingerprint headers — never leak to an Anthropic upstream ---
            // These are client/session identifiers from the incoming Codex request,
            // not Anthropic protocol headers. Forwarding them both leaks identity and
            // can defeat strict gateway fingerprint checks.
            // The full set lives in `is_codex_client_fingerprint_header` so it stays in one
            // place. (HeaderName is lowercased by the http crate, so a direct match is safe.)
            // Codex Stack 请求发往第三方时同样剥掉：官方做路由时 Codex 每个请求都带着 ChatGPT
            // 身份（`chatgpt-account-id` 等），它们只能发往官方上游。Stack 目标不会是官方账号，
            // 这里仍按上游判断，防止以后放宽。只限 Codex：Claude Code 自己的 `x-stainless-*`
            // 是 Anthropic SDK 的正常请求头，Stack 请求照常转发。
            if should_strip_codex_client_fingerprint_header(
                key_str,
                codex_responses_to_anthropic || chat_to_anthropic,
            ) {
                continue;
            }

            // --- accept — force application/json on the Codex/Chat→Anthropic path ---
            // The clients send `Accept: text/event-stream` when streaming, whereas
            // a native Anthropic client sends `application/json` (streaming is driven
            // by the body's stream:true). Strict Anthropic gateways return 406 Not
            // Acceptable for an event-stream Accept, so normalize it here.
            if (codex_responses_to_anthropic || chat_to_anthropic)
                && key_str.eq_ignore_ascii_case("accept")
            {
                if !saw_accept {
                    saw_accept = true;
                    ordered_headers.append(
                        http::header::ACCEPT,
                        http::HeaderValue::from_static("application/json"),
                    );
                }
                continue;
            }

            // --- accept-encoding — transform / SSE 路径强制 identity，其余保留原值 ---
            if key_str.eq_ignore_ascii_case("accept-encoding") {
                if !saw_accept_encoding {
                    saw_accept_encoding = true;
                    if force_identity_encoding {
                        ordered_headers.append(
                            http::header::ACCEPT_ENCODING,
                            http::HeaderValue::from_static("identity"),
                        );
                    } else {
                        ordered_headers.append(key.clone(), value.clone());
                    }
                }
                continue;
            }

            // --- user-agent: provider-level override for local proxy routing ---
            if !is_copilot && key_str.eq_ignore_ascii_case("user-agent") {
                if !saw_user_agent {
                    saw_user_agent = true;
                    if let Some(ref ua) = custom_user_agent {
                        ordered_headers.append(http::header::USER_AGENT, ua.clone());
                    } else {
                        ordered_headers.append(key.clone(), value.clone());
                    }
                }
                continue;
            }

            // --- anthropic-beta — 用重建值替换（确保含 claude-code 标记） ---
            if key_str.eq_ignore_ascii_case("anthropic-beta") {
                if !saw_anthropic_beta {
                    saw_anthropic_beta = true;
                    if let Some(ref beta_val) = anthropic_beta_value {
                        if let Ok(hv) = http::HeaderValue::from_str(beta_val) {
                            ordered_headers.append("anthropic-beta", hv);
                        }
                    }
                }
                continue;
            }

            // --- anthropic-version — 透传客户端值 ---
            if key_str.eq_ignore_ascii_case("anthropic-version") {
                if should_send_anthropic_headers {
                    saw_anthropic_version = true;
                    ordered_headers.append(key.clone(), value.clone());
                }
                continue;
            }

            // --- Copilot 指纹头 — 跳过（由 auth_headers 提供） ---
            if copilot_fingerprint_headers
                .iter()
                .any(|h| key_str.eq_ignore_ascii_case(h))
            {
                continue;
            }

            // --- 默认：透传 ---
            ordered_headers.append(key.clone(), value.clone());
        }

        // 如果原始请求中没有认证头，在末尾追加
        if !saw_auth && !auth_headers.is_empty() {
            for (ah_name, ah_value) in &auth_headers {
                ordered_headers.append(ah_name.clone(), ah_value.clone());
            }
        }

        // transform / SSE 路径在缺失时补 identity；普通透传不主动补 accept-encoding
        if !saw_accept_encoding && force_identity_encoding {
            ordered_headers.append(
                http::header::ACCEPT_ENCODING,
                http::HeaderValue::from_static("identity"),
            );
        }

        // On the Codex/Chat→Anthropic path, add application/json when Accept is missing
        if (codex_responses_to_anthropic || chat_to_anthropic) && !saw_accept {
            ordered_headers.append(
                http::header::ACCEPT,
                http::HeaderValue::from_static("application/json"),
            );
        }

        // Codex→Anthropic emulation: inject Claude Code's x-app: cli
        if codex_impersonate_claude_code {
            ordered_headers.append("x-app", http::HeaderValue::from_static("cli"));
        }

        if !saw_user_agent {
            if let Some(ua) = custom_user_agent
                .as_ref()
                .or(opencode_fallback_user_agent.as_ref())
            {
                ordered_headers.append(http::header::USER_AGENT, ua.clone());
            }
        }

        // 如果原始请求中没有 anthropic-beta 且有值需要添加，追加
        if !saw_anthropic_beta {
            if let Some(ref beta_val) = anthropic_beta_value {
                if let Ok(hv) = http::HeaderValue::from_str(beta_val) {
                    ordered_headers.append("anthropic-beta", hv);
                }
            }
        }

        // anthropic-version: add the default only when it is missing.
        // The Codex→Anthropic path also needs this header. Note this is independent
        // of anthropic-beta: the Claude Code-specific beta is only sent when
        // impersonation is on (handled above); on the plain Codex→Anthropic path
        // (impersonation off) anthropic-version is still required but no beta is sent.
        if (should_send_anthropic_headers || codex_responses_to_anthropic) && !saw_anthropic_version
        {
            ordered_headers.append(
                "anthropic-version",
                http::HeaderValue::from_static("2023-06-01"),
            );
        }

        // Codex OAuth 反代尽量对齐官方 Codex CLI 的会话路由信号。
        // 只发送客户端提供的 session_id；生成的 UUID 每次不同，反而会破坏前缀缓存。
        for (name, value) in Vec::<(http::HeaderName, http::HeaderValue)>::new() {
            ordered_headers.insert(name, value);
        }

        // 序列化请求体。GET/HEAD 是 idempotent/safe 方法，按 HTTP 语义不应携带 body；
        // 强行附带 JSON body 会让某些上游（如 Google Gemini 的 models.list）拒绝请求。
        let body_bytes = if matches!(method, &http::Method::GET | &http::Method::HEAD) {
            Vec::new()
        } else {
            serde_json::to_vec(&filtered_body).map_err(|e| {
                ProxyError::Internal(format!("Failed to serialize request body: {e}"))
            })?
        };

        // 确保 content-type 存在
        if !ordered_headers.contains_key(http::header::CONTENT_TYPE) {
            ordered_headers.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static("application/json"),
            );
        }

        apply_local_proxy_header_overrides(
            &mut ordered_headers,
            provider
                .meta
                .as_ref()
                .and_then(|meta| meta.local_proxy_request_overrides.as_ref()),
            is_copilot,
        );

        // x-account-id 注入已随托管账号功能一起移除。

        // 日志目标 URL 的脱敏分两种情形：
        // - 有已知密钥(log_secrets 非空)：记录脱敏后的完整 URL，剥 userinfo/query
        //   并抹掉已知密钥值，保留 host+path 便于诊断 base_url 配错路径导致的 404。
        // - 无已知密钥：凭据可能整个内嵌在 path 里且无从脱敏，只记 origin，
        //   避免默认 Info 级把形如 https://gw/<KEY>/v1 的 path 完整落盘。
        let target_for_log = if log_secrets.is_empty() {
            crate::redact_url_origin_for_log(&url)
        } else {
            crate::redact_url_for_log_with_secrets(&url, &log_secrets)
        };

        // 输出请求信息日志
        let tag = adapter.name();
        let request_model = filtered_body
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("<none>");
        log::info!("[{tag}] >>> 请求目标: {target_for_log} (model={request_model})");
        log::debug!(
            "[{tag}] >>> 请求体已准备: bytes={}, hash={} (content omitted)",
            body_bytes.len(),
            short_value_hash(Some(&filtered_body))
        );

        // 序列化后释放工作副本；只有加密子任务的流式判错还需要保留请求体。
        let encrypted_agent_request: Option<Value> = None;

        // 确定超时
        let timeout = if self.non_streaming_timeout.is_zero() {
            std::time::Duration::from_secs(600) // 默认 600 秒
        } else {
            self.non_streaming_timeout
        };

        // 获取全局代理 URL（上游是本机地址时直连，不送进代理）
        let upstream_proxy_url: Option<String> = super::http_client::proxy_url_for(&url);

        // SOCKS5 代理不支持 CONNECT 隧道，需要用 reqwest
        let is_socks_proxy = upstream_proxy_url
            .as_deref()
            .map(|u| u.starts_with("socks5"))
            .unwrap_or(false);

        let preserve_exact_header_case = should_preserve_exact_header_case(
            adapter.name(),
            resolved_claude_api_format.as_deref(),
        );

        // 发送请求
        let response = if is_socks_proxy || !preserve_exact_header_case {
            // OpenAI / Copilot / Codex 类后端不依赖原始 header 大小写；走 reqwest
            // 连接池，避免 raw TCP/TLS path 每次请求都重新握手。SOCKS5 也只能走 reqwest。
            log::debug!(
                "[Forwarder] Using pooled reqwest client (preserve_exact_header_case={preserve_exact_header_case}, socks_proxy={is_socks_proxy})"
            );
            let client = super::http_client::get_for_url(&url);
            let mut request = client.request(method.clone(), &url);
            if request_is_streaming {
                // reqwest 的 timeout 是整请求超时；流式请求交给 response_processor
                // 的首包/静默期超时控制，避免长流被总时长误杀。
                request = request.timeout(std::time::Duration::from_secs(24 * 60 * 60));
            } else if !self.non_streaming_timeout.is_zero() {
                request = request.timeout(self.non_streaming_timeout);
            }
            for (key, value) in &ordered_headers {
                request = request.header(key, value);
            }
            let send = request.body(body_bytes).send();
            let send_result = if request_is_streaming {
                let header_timeout = if self.streaming_first_byte_timeout.is_zero() {
                    timeout
                } else {
                    self.streaming_first_byte_timeout
                };
                tokio::time::timeout(header_timeout, send)
                    .await
                    .map_err(|_| {
                        ProxyError::Timeout(format!(
                            "流式响应首包超时: {}s（上游未返回响应头）",
                            header_timeout.as_secs()
                        ))
                    })?
            } else {
                send.await
            };
            let reqwest_resp = send_result.map_err(map_reqwest_send_error)?;
            ProxyResponse::Reqwest(reqwest_resp)
        } else {
            // HTTP 代理或直连：走 hyper raw write（保持 header 大小写）
            // 如果有 HTTP 代理，hyper_client 会用 CONNECT 隧道穿过代理
            let uri: http::Uri = url.parse().map_err(|e| {
                ProxyError::ForwardFailed(format!("Invalid upstream URL ({target_for_log}): {e}"))
            })?;
            super::hyper_client::send_request(
                uri,
                &target_for_log,
                method.clone(),
                ordered_headers,
                extensions.clone(),
                body_bytes,
                timeout,
                upstream_proxy_url.as_deref(),
            )
            .await?
        };

        // 检查响应状态
        let status = response.status();

        if status.is_success() {
            let mut response = self
                .prepare_success_response_for_failover(response, request_is_streaming)
                .await?;
            // Streaming requests normally return SSE. If a compatible gateway
            // explicitly returns JSON instead, buffer and validate it inside the retry
            // loop as well so a 2xx Anthropic error envelope can still fail over. Do
            // not buffer unknown content types: some gateways omit the SSE header.
            if codex_responses_to_anthropic && (!request_is_streaming || response.is_json()) {
                response = self
                    .validate_codex_anthropic_success_response(response)
                    .await?;
            } else if matches!(
                resolved_claude_api_format.as_deref(),
                Some("openai_responses")
            ) {
                if !request_is_streaming || response.is_json() {
                    // Claude→Responses gateways can also return a semantic failure in an
                    // HTTP 2xx Response object. Validate buffered/JSON bodies inside the
                    // retry loop so an early failure can still select another provider.
                    response = self.validate_responses_success_response(response).await?;
                } else {
                    // Delay committing the downstream stream until the upstream emits
                    // either productive output or a valid non-failure terminal event.
                    // A response.failed/error before output remains failover-safe.
                    response = self.validate_responses_stream_start(response).await?;
                }
            } else if let Some(request) = encrypted_agent_request
                .as_ref()
                .filter(|_| !response.is_json())
            {
                response = self
                    .hold_encrypted_agent_task_stream(response, request)
                    .await?;
            }
            Ok((response, resolved_claude_api_format, outbound_model))
        } else {
            let status_code = status.as_u16();
            // 错误响应同样可能被上游压缩（content-encoding）。reqwest 未启用任何
            // 自动解压 feature，这里拿到的是原始字节；不解压的话，压缩过的错误体会
            // 在 from_utf8 处变成非 UTF-8 而被丢弃，隐藏掉上游的限流/鉴权等详情。
            let encoding = get_content_encoding(response.headers());
            let raw = response.bytes_with_limit(MAX_RESPONSE_BODY_BYTES).await?;
            let decoded = match encoding {
                Some(encoding) => {
                    match decompress_body_with_limit(&encoding, &raw, MAX_RESPONSE_BODY_BYTES) {
                        Ok(Some(decompressed)) => decompressed,
                        // 不支持的编码 / 解压失败 / 解压后超限：退回（已有上限的）
                        // 原始字节，尽量保留可读信息
                        _ => raw.to_vec(),
                    }
                }
                None => raw.to_vec(),
            };
            let body_text = String::from_utf8(decoded).ok();

            Err(ProxyError::UpstreamError {
                status: status_code,
                body: body_text,
            })
        }
    }

    /// 故障转移开启时，成功不能只看上游响应头。
    ///
    /// - 非流式：先把完整 body 读到内存，读超时/连接中断会回到 retry loop 尝试下一家。
    /// - 流式：至少等首个 chunk 到达，避免上游返回 200 后一直不吐 SSE 时被误记成功。
    async fn prepare_success_response_for_failover(
        &self,
        response: ProxyResponse,
        request_is_streaming: bool,
    ) -> Result<ProxyResponse, ProxyError> {
        if request_is_streaming {
            return self.prime_streaming_response(response).await;
        }

        if self.non_streaming_timeout.is_zero() {
            return Ok(response);
        }

        let status = response.status();
        let headers = response.headers().clone();
        let body_timeout = self.non_streaming_timeout;
        let body = tokio::time::timeout(
            body_timeout,
            response.bytes_with_limit(MAX_RESPONSE_BODY_BYTES),
        )
        .await
        .map_err(|_| {
            ProxyError::Timeout(format!(
                "响应体读取超时: {}s（上游发完响应头后 body 未到达）",
                body_timeout.as_secs()
            ))
        })??;

        Ok(ProxyResponse::buffered(status, headers, body))
    }

    /// Some Anthropic-compatible gateways return an Anthropic error envelope with
    /// HTTP 2xx. Validate it inside the retry loop so the request can fail over to
    /// the next provider; the response transformer runs too late for that.
    async fn validate_codex_anthropic_success_response(
        &self,
        response: ProxyResponse,
    ) -> Result<ProxyResponse, ProxyError> {
        let status = response.status();
        let headers = response.headers().clone();
        let encoding = get_content_encoding(&headers);
        let raw = response.bytes_with_limit(MAX_RESPONSE_BODY_BYTES).await?;
        let decoded = match encoding {
            Some(encoding) => {
                match decompress_body_with_limit(&encoding, &raw, MAX_RESPONSE_BODY_BYTES) {
                    Ok(Some(decompressed)) => decompressed,
                    _ => raw.to_vec(),
                }
            }
            None => raw.to_vec(),
        };

        if let Some(message) = codex_anthropic_error_envelope_message(&decoded) {
            return Err(ProxyError::TransformError(format!(
                "Anthropic upstream returned a 2xx error envelope: {message}"
            )));
        }

        Ok(ProxyResponse::buffered(status, headers, raw))
    }

    async fn validate_responses_success_response(
        &self,
        response: ProxyResponse,
    ) -> Result<ProxyResponse, ProxyError> {
        let status = response.status();
        let headers = response.headers().clone();
        let encoding = get_content_encoding(&headers);
        let raw = response.bytes_with_limit(MAX_RESPONSE_BODY_BYTES).await?;
        let decoded = match encoding {
            Some(encoding) => {
                match decompress_body_with_limit(&encoding, &raw, MAX_RESPONSE_BODY_BYTES) {
                    Ok(Some(decompressed)) => decompressed,
                    _ => raw.to_vec(),
                }
            }
            None => raw.to_vec(),
        };

        if let Some(message) = responses_error_envelope_message(&decoded) {
            return Err(ProxyError::TransformError(format!(
                "Responses upstream returned a 2xx failure: {message}"
            )));
        }

        Ok(ProxyResponse::buffered(status, headers, raw))
    }

    async fn validate_responses_stream_start(
        &self,
        response: ProxyResponse,
    ) -> Result<ProxyResponse, ProxyError> {
        const MAX_PRIME_BYTES: usize = 256 * 1024;

        let status = response.status();
        let headers = response.headers().clone();
        let mut stream = Box::pin(response.bytes_stream());
        let mut replay_chunks: Vec<Bytes> = Vec::new();
        let mut sse = crate::proxy::sse::SseBuffer::new();

        loop {
            let next = if self.streaming_first_byte_timeout.is_zero() {
                stream.next().await
            } else {
                tokio::time::timeout(self.streaming_first_byte_timeout, stream.next())
                    .await
                    .map_err(|_| {
                        ProxyError::Timeout(format!(
                            "Responses stream produced no semantic output within {}s",
                            self.streaming_first_byte_timeout.as_secs()
                        ))
                    })?
            };

            let Some(chunk) = next else {
                if let Some(outcome) = inspect_responses_json_document(sse.pending()) {
                    outcome?;
                    let replay = futures::stream::iter(replay_chunks.into_iter().map(Ok));
                    return Ok(ProxyResponse::streamed(status, headers, replay));
                }
                if !sse.pending().trim().is_empty() {
                    if let Some(outcome) = inspect_responses_start_event(sse.pending().trim()) {
                        outcome?;
                        let replay = futures::stream::iter(replay_chunks.into_iter().map(Ok));
                        return Ok(ProxyResponse::streamed(status, headers, replay));
                    }
                }
                return Err(ProxyError::ForwardFailed(
                    "Responses stream ended before producing output or a terminal event"
                        .to_string(),
                ));
            };
            let chunk = chunk.map_err(|error| {
                ProxyError::ForwardFailed(format!(
                    "Failed while validating Responses stream start: {error}"
                ))
            })?;
            sse.push_bytes(&chunk);
            replay_chunks.push(chunk);

            // Some compatible gateways ignore `stream:true` and return a complete
            // Responses JSON document without a JSON content-type. Recognize that
            // shape before looking for SSE delimiters; pretty-printed JSON may itself
            // contain blank lines and must stay intact.
            if let Some(outcome) = inspect_responses_json_document(sse.pending()) {
                outcome?;
                let replay = futures::stream::iter(replay_chunks.into_iter().map(Ok)).chain(stream);
                return Ok(ProxyResponse::streamed(status, headers, replay));
            }

            while let Some(block) = sse.next_block() {
                if let Some(outcome) = inspect_responses_start_event(block) {
                    outcome?;
                    let replay =
                        futures::stream::iter(replay_chunks.into_iter().map(Ok)).chain(stream);
                    return Ok(ProxyResponse::streamed(status, headers, replay));
                }
            }

            if replay_chunks.iter().map(Bytes::len).sum::<usize>() >= MAX_PRIME_BYTES {
                log::warn!(
                    "[Claude/Responses] semantic stream priming exceeded {MAX_PRIME_BYTES} bytes; committing buffered stream"
                );
                let replay = futures::stream::iter(replay_chunks.into_iter().map(Ok)).chain(stream);
                return Ok(ProxyResponse::streamed(status, headers, replay));
            }
        }
    }

    /// 原样转发的 Responses 请求末尾是一条带密文的子 agent 任务时（见
    /// `opaque_state_rectifier::carries_encrypted_agent_task`），先看到第一个不是生命周期的事件
    /// 再交出流：上游解不开密文时会在流里、输出之前回 `response.failed`，这时改成上游错误，
    /// 回到重试循环里和 HTTP 报错一样整流或说明原因。其余情况（有输出、别的失败、到了截止
    /// 时间、缓冲太多）把读过的字节原样接回去，客户端看到的流和没经过这里一样。
    ///
    /// 整个缓冲阶段只有一个截止时间（不是每读一块重新计时）：上游在输出前一直发心跳时，流
    /// 也得按时交出去。解不开密文是上游一收到请求就报的错，不用等到生成，所以最多压
    /// [`MAX_HOLD`]，首包超时更短时按首包超时。
    async fn hold_encrypted_agent_task_stream(
        &self,
        response: ProxyResponse,
        request: &Value,
    ) -> Result<ProxyResponse, ProxyError> {
        const MAX_HOLD_BYTES: usize = 256 * 1024;
        const MAX_HOLD: std::time::Duration = std::time::Duration::from_secs(10);

        let window = match self.streaming_first_byte_timeout {
            timeout if timeout.is_zero() => MAX_HOLD,
            timeout => timeout.min(MAX_HOLD),
        };
        let deadline = tokio::time::Instant::now() + window;
        let status = response.status();
        let headers = response.headers().clone();
        let mut stream = Box::pin(response.bytes_stream());
        let mut replay_chunks: Vec<Bytes> = Vec::new();
        let mut held_bytes = 0usize;
        let mut sse = crate::proxy::sse::SseBuffer::new();

        'hold: loop {
            let Ok(next) = tokio::time::timeout_at(deadline, stream.next()).await else {
                break 'hold;
            };
            let Some(chunk) = next else {
                break 'hold;
            };
            // 还没有交出流，读出错和首包前出错一样回到重试循环。
            let chunk = chunk
                .map_err(|error| ProxyError::ForwardFailed(format!("读取流式响应失败: {error}")))?;
            sse.push_bytes(&chunk);
            held_bytes += chunk.len();
            replay_chunks.push(chunk);

            while let Some(block) = sse.next_block() {
                match encrypted_task_stream_start(block, request) {
                    StreamStart::Pending => {}
                    StreamStart::Commit => break 'hold,
                    StreamStart::Rejected(error) => return Err(error),
                }
            }
            if held_bytes > MAX_HOLD_BYTES {
                break 'hold;
            }
        }

        let replay = futures::stream::iter(replay_chunks.into_iter().map(Ok)).chain(stream);
        Ok(ProxyResponse::streamed(status, headers, replay))
    }

    async fn prime_streaming_response(
        &self,
        response: ProxyResponse,
    ) -> Result<ProxyResponse, ProxyError> {
        if self.streaming_first_byte_timeout.is_zero() {
            return Ok(response);
        }

        let status = response.status();
        let headers = response.headers().clone();
        let timeout = self.streaming_first_byte_timeout;
        let mut stream = Box::pin(response.bytes_stream());

        let first = tokio::time::timeout(timeout, stream.next())
            .await
            .map_err(|_| {
                ProxyError::Timeout(format!(
                    "流式响应首包超时: {}s（上游已返回响应头但未返回数据）",
                    timeout.as_secs()
                ))
            })?;

        let Some(first) = first else {
            return Err(ProxyError::ForwardFailed(
                "流式响应在首包到达前结束".to_string(),
            ));
        };

        let first =
            first.map_err(|e| ProxyError::ForwardFailed(format!("读取流式响应首包失败: {e}")))?;

        let replay = futures::stream::once(async move { Ok(first) }).chain(stream);
        Ok(ProxyResponse::streamed(status, headers, replay))
    }

    async fn resolve_claude_api_format(&self, provider: &Provider, _body: &Value) -> String {
        super::providers::get_claude_api_format(provider).to_string()
    }





    fn categorize_proxy_error(&self, error: &ProxyError, _provider: &Provider) -> ErrorCategory {
        match error {
            // 网络和上游错误：都应该尝试下一个供应商
            ProxyError::Timeout(_) => ErrorCategory::Retryable,
            ProxyError::ForwardFailed(_) => ErrorCategory::Retryable,
            ProxyError::ProviderUnhealthy(_) => ErrorCategory::Retryable,
            // 上游 HTTP 错误：按状态码分桶。
            //
            // 客户端请求自身有问题的状态码无论换哪个 provider 都会被拒绝，
            // 继续轮询只会放大错误率、污染熔断器健康度、浪费配额：
            //   400 Bad Request / 422 Unprocessable Entity   ← 请求体格式或语义错误
            //   405 Method Not Allowed / 406 Not Acceptable  ← 方法或 Accept 错误
            //   413 Payload Too Large / 414 URI Too Long     ← 客户端构造超限
            //   415 Unsupported Media Type                    ← Content-Type 错误
            //   501 Not Implemented                           ← 上游协议确实不支持
            //
            // 其他 4xx（401/403/404/408/409/429/451 等）和全部 5xx 都保留
            // Retryable —— 换一家 provider 可能持有不同的 key、配额、地域或模型映射。
            ProxyError::UpstreamError { status, .. } => match *status {
                400 | 405 | 406 | 413 | 414 | 415 | 422 | 501 => ErrorCategory::NonRetryable,
                _ => ErrorCategory::Retryable,
            },
            // Provider 级配置/转换问题：换一个 Provider 可能就能成功
            ProxyError::ConfigError(_) => ErrorCategory::Retryable,
            ProxyError::TransformError(_) => ErrorCategory::Retryable,
            ProxyError::AuthError(_) => ErrorCategory::Retryable,
            ProxyError::StreamIdleTimeout(_) => ErrorCategory::Retryable,
            // 无可用供应商：所有供应商都试过了，无法重试
            ProxyError::NoAvailableProvider => ErrorCategory::NonRetryable,
            // 其他错误（数据库/内部错误等）：不是换供应商能解决的问题
            _ => ErrorCategory::NonRetryable,
        }
    }
}

/// 从 ProxyError 中提取错误消息
fn extract_error_message(error: &ProxyError) -> Option<String> {
    match error {
        ProxyError::UpstreamError { body, .. } => body.clone(),
        _ => Some(error.to_string()),
    }
}

/// 检测 Provider 是否为 Bedrock（通过 CLAUDE_CODE_USE_BEDROCK 环境变量判断）
fn is_bedrock_provider(provider: &Provider) -> bool {
    provider
        .settings_config
        .get("env")
        .and_then(|e| e.get("CLAUDE_CODE_USE_BEDROCK"))
        .and_then(|v| v.as_str())
        .map(|v| v == "1")
        .unwrap_or(false)
}

fn refresh_success_rate(status: &mut ProxyStatus) {
    if status.total_requests > 0 {
        status.success_rate =
            (status.success_requests as f32 / status.total_requests as f32) * 100.0;
    }
}

fn build_retryable_failure_log(
    provider_name: &str,
    attempted_providers: usize,
    total_providers: usize,
    error: &ProxyError,
) -> (&'static str, String) {
    let error_summary = summarize_proxy_error(error);

    if total_providers <= 1 {
        (
            log_fwd::SINGLE_PROVIDER_FAILED,
            format!("Provider {provider_name} 请求失败: {error_summary}"),
        )
    } else {
        (
            log_fwd::PROVIDER_FAILED_RETRY,
            format!(
                "Provider {provider_name} 失败，继续尝试下一个 ({attempted_providers}/{total_providers}): {error_summary}"
            ),
        )
    }
}

fn build_terminal_failure_log(
    attempted_providers: usize,
    total_providers: usize,
    last_error: Option<&ProxyError>,
) -> Option<(&'static str, String)> {
    if total_providers <= 1 {
        return None;
    }

    let error_summary = last_error
        .map(summarize_proxy_error)
        .unwrap_or_else(|| "未知错误".to_string());

    Some((
        log_fwd::ALL_PROVIDERS_FAILED,
        format!(
            "已尝试 {attempted_providers}/{total_providers} 个 Provider，均失败。最后错误: {error_summary}"
        ),
    ))
}

fn summarize_proxy_error(error: &ProxyError) -> String {
    match error {
        ProxyError::UpstreamError { status, body } => {
            let body_summary = body
                .as_deref()
                .map(summarize_upstream_body)
                .filter(|summary| !summary.is_empty());

            match body_summary {
                Some(summary) => format!("上游 HTTP {status}: {summary}"),
                None => format!("上游 HTTP {status}"),
            }
        }
        ProxyError::Timeout(message) => {
            format!("请求超时: {}", summarize_text_for_log(message, 180))
        }
        ProxyError::ForwardFailed(message) => {
            format!("请求转发失败: {}", summarize_text_for_log(message, 180))
        }
        ProxyError::TransformError(message) => {
            format!("响应转换失败: {}", summarize_text_for_log(message, 180))
        }
        ProxyError::ConfigError(message) => {
            format!("配置错误: {}", summarize_text_for_log(message, 180))
        }
        ProxyError::AuthError(message) => {
            format!("认证失败: {}", summarize_text_for_log(message, 180))
        }
        _ => summarize_text_for_log(&error.to_string(), 180),
    }
}

fn summarize_upstream_body(body: &str) -> String {
    if let Ok(json_body) = serde_json::from_str::<Value>(body) {
        if let Some(message) = extract_json_error_message(&json_body) {
            return summarize_text_for_log(&message, 180);
        }

        if let Ok(compact_json) = serde_json::to_string(&json_body) {
            return summarize_text_for_log(&compact_json, 180);
        }
    }

    summarize_text_for_log(body, 180)
}

fn extract_json_error_message(body: &Value) -> Option<String> {
    let candidates = [
        body.pointer("/error/message"),
        body.pointer("/message"),
        body.pointer("/detail"),
        body.pointer("/error"),
    ];

    candidates
        .into_iter()
        .flatten()
        .find_map(|value| value.as_str().map(ToString::to_string))
}

fn split_endpoint_and_query(endpoint: &str) -> (&str, Option<&str>) {
    endpoint
        .split_once('?')
        .map_or((endpoint, None), |(path, query)| (path, Some(query)))
}

fn strip_beta_query(query: Option<&str>) -> Option<String> {
    let filtered = query.map(|query| {
        query
            .split('&')
            .filter(|pair| !pair.is_empty() && !pair.starts_with("beta="))
            .collect::<Vec<_>>()
            .join("&")
    });

    match filtered.as_deref() {
        Some("") | None => None,
        Some(_) => filtered,
    }
}

fn is_claude_messages_path(path: &str) -> bool {
    matches!(path, "/v1/messages" | "/claude/v1/messages")
}

/// Chat 入口 → Responses 上游：`/chat/completions` 改成 `/responses`。
fn rewrite_chat_endpoint_to_responses(endpoint: &str) -> (String, Option<String>) {
    let (path, query) = split_endpoint_and_query(endpoint);
    let rewritten = match path.strip_suffix("/chat/completions") {
        Some(prefix) => format!("{prefix}/responses"),
        None => path.to_string(),
    };
    (rewritten, query.map(ToString::to_string))
}

/// Chat 入口 → Anthropic 上游：`/chat/completions` 改成 `/v1/messages`。
fn rewrite_chat_endpoint_to_anthropic(endpoint: &str) -> (String, Option<String>) {
    let (_path, query) = split_endpoint_and_query(endpoint);
    let passthrough_query = query.map(ToString::to_string);
    let target_path = "/v1/messages";
    let rewritten = match passthrough_query.as_deref() {
        Some(query) if !query.is_empty() => format!("{target_path}?{query}"),
        _ => target_path.to_string(),
    };
    (rewritten, passthrough_query)
}

/// 供应商把完整端点写进 base URL 时，把 `/chat/completions` 换成 `/responses`。
fn rewrite_base_url_chat_to_responses(base_url: &str) -> String {
    let trimmed = base_url.trim();
    let (path, query) = match trimmed.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (trimmed, None),
    };
    let rewritten = match path.trim_end_matches('/').strip_suffix("/chat/completions") {
        Some(prefix) => format!("{prefix}/responses"),
        None => path.to_string(),
    };
    match query {
        Some(query) => format!("{rewritten}?{query}"),
        None => rewritten,
    }
}

fn rewrite_codex_responses_endpoint_to_chat(endpoint: &str) -> (String, Option<String>) {
    let (_path, query) = split_endpoint_and_query(endpoint);
    let passthrough_query = query.map(ToString::to_string);
    let target_path = "/chat/completions";
    let rewritten = match passthrough_query.as_deref() {
        Some(query) if !query.is_empty() => format!("{target_path}?{query}"),
        _ => target_path.to_string(),
    };

    (rewritten, passthrough_query)
}

/// Claude Code client fingerprint (used for Codex→Anthropic emulation to pass a
/// gateway's "Claude Code only" check).
const CLAUDE_CODE_USER_AGENT: &str = "claude-cli/1.0.119 (external, cli)";
const CLAUDE_CODE_SYSTEM_IDENTITY: &str =
    "You are Claude Code, Anthropic's official CLI for Claude.";

/// Insert the Claude Code identity as the first line before the `system` field in
/// the Anthropic request body.
///
/// Anthropic subscription/OAuth plans require the first system block to be exactly
/// this identity line. After conversion `system` is a string (from Codex
/// instructions); normalize it into an array here: [identity line, original system...].
fn prepend_claude_code_system_prompt(body: &mut Value) {
    let identity = serde_json::json!({ "type": "text", "text": CLAUDE_CODE_SYSTEM_IDENTITY });
    let mut blocks: Vec<Value> = vec![identity];
    match body.get("system") {
        Some(Value::String(existing)) if !existing.is_empty() => {
            blocks.push(serde_json::json!({ "type": "text", "text": existing }));
        }
        Some(Value::Array(existing)) => {
            // Idempotent: skip re-injection if the first block is already the identity line.
            if existing
                .first()
                .and_then(|b| b.get("text"))
                .and_then(|t| t.as_str())
                == Some(CLAUDE_CODE_SYSTEM_IDENTITY)
            {
                return;
            }
            blocks.extend(existing.iter().cloned());
        }
        _ => {}
    }
    body["system"] = Value::Array(blocks);
}

/// Headers a native Claude Code client never sends but the Codex/OpenAI CLI (and its
/// stainless SDK layer) do. Dropped for every Codex→Anthropic request so the upstream sees a
/// clean Anthropic client fingerprint. Centralized here so the set stays in one place and future
/// additions can't miss a code path. `key_str` is already lowercased by the http crate.
/// Whether `base_url` already ends in `endpoint_suffix` (e.g. `/v1/messages` or
/// `/chat/completions`), ignoring surrounding whitespace, any `?query`/`#fragment`, and a
/// trailing slash. Used to avoid double-appending the endpoint when a user pastes a full
/// URL but leaves the "full URL" switch off (`.../v1/messages` → `.../v1/messages/v1/messages`,
/// a non-retryable 400). `endpoint_suffix` must be lowercase.
fn base_url_is_full_endpoint(base_url: &str, endpoint_suffix: &str) -> bool {
    let trimmed = base_url.trim();
    // Match against the path only: a `?query`/`#fragment` on a full endpoint URL must not
    // hide the suffix (`.../v1/messages?beta=true` still ends in the endpoint).
    let path = match trimmed.split_once(['?', '#']) {
        Some((head, _)) => head,
        None => trimmed,
    };
    path.trim_end_matches('/')
        .to_ascii_lowercase()
        .ends_with(endpoint_suffix)
}

fn is_codex_client_fingerprint_header(key_str: &str) -> bool {
    matches!(
        key_str,
        "originator"
            | "session_id"
            | "session-id"
            | "thread-id"
            | "conversation_id"
            | "chatgpt-account-id"
            | "x-client-request-id"
            | "openai-beta"
            | "openai-organization"
            | "openai-project"
            // Codex 内置 openai 的固定请求头（值是 Codex 版本号）。
            | "version"
    ) || key_str.starts_with("x-stainless-")
        || key_str.starts_with("x-codex-")
        // ChatGPT 登录态下 Codex 每个请求都带：`x-oai-attestation`（设备证明）、
        // `x-openai-fedramp`、`x-openai-account-routing-override`、`x-openai-memgen-request`、
        // `x-openai-internal-codex-responses-lite`、`x-responsesapi-include-timing-metrics`。
        || key_str.starts_with("x-oai-")
        || key_str.starts_with("x-openai-")
        || key_str.starts_with("x-responsesapi-")
}

/// Decide whether a Codex fingerprint header must be removed before the request
/// reaches an upstream.
///
/// Only the Codex→Anthropic conversion needs it: a native Claude client never
/// sends these headers, so a converted request must not either.
fn should_strip_codex_client_fingerprint_header(
    key_str: &str,
    codex_responses_to_anthropic: bool,
) -> bool {
    codex_responses_to_anthropic && is_codex_client_fingerprint_header(key_str)
}

fn codex_anthropic_error_envelope_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("error") && value.get("error").is_none() {
        return None;
    }
    let error = value.get("error").unwrap_or(&value);
    let error_type = error.get("type").and_then(Value::as_str).unwrap_or("error");
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| error.to_string());
    Some(format!("{error_type}: {message}"))
}

fn responses_error_envelope_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let status = value.get("status").and_then(Value::as_str);
    let has_error = value.get("error").is_some_and(|error| !error.is_null());
    if !matches!(status, Some("failed" | "cancelled")) && !has_error {
        return None;
    }

    let error = value.get("error").unwrap_or(&value);
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| error.get("code").and_then(Value::as_str))
        .unwrap_or_else(|| status.unwrap_or("error"));
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(match status {
            Some("cancelled") => "response generation was cancelled",
            _ => "response generation failed",
        });
    Some(format!("{error_type}: {message}"))
}

/// Prompt caching is part of the Codex→Anthropic protocol bridge rather than an
/// optional Bedrock optimizer. Codex requests do not contain Anthropic
/// `cache_control`, so keep bridge caching on by default while still honoring the
/// dedicated cache-injection switch. Injected breakpoints always use Anthropic's
/// standard 5-minute TTL.
fn codex_anthropic_cache_config(config: &OptimizerConfig) -> OptimizerConfig {
    OptimizerConfig {
        enabled: true,
        thinking_optimizer: false,
        cache_injection: config.cache_injection,
    }
}

/// [`Forwarder::hold_encrypted_agent_task_stream`] 看到一个完整 SSE 块之后怎么做。
enum StreamStart {
    /// 生命周期事件（或不是 JSON），接着读。
    Pending,
    /// 有输出或别的结局：交出流。
    Commit,
    /// 输出之前上游就自证验不了请求里的密文。
    Rejected(ProxyError),
}

fn encrypted_task_stream_start(block: &str, request: &Value) -> StreamStart {
    let mut named_event = None;
    let mut data_lines = Vec::new();
    for line in block.lines() {
        if let Some(event) = crate::proxy::sse::strip_sse_field(line, "event") {
            named_event = Some(event.trim().to_string());
        } else if let Some(data) = crate::proxy::sse::strip_sse_field(line, "data") {
            data_lines.push(data);
        }
    }
    let Ok(value) = serde_json::from_str::<Value>(&data_lines.join("\n")) else {
        return StreamStart::Pending;
    };
    let event = named_event
        .as_deref()
        .filter(|event| !event.is_empty())
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");
    match event {
        "response.created" | "response.in_progress" | "response.queued" => StreamStart::Pending,
        "response.failed" | "error" => {
            let response = value.get("response").unwrap_or(&value);
            let error = response.get("error").unwrap_or(response);
            match super::opaque_state_rectifier::in_stream_rejection(error, request) {
                Some(rejection) => StreamStart::Rejected(rejection),
                None => StreamStart::Commit,
            }
        }
        _ => StreamStart::Commit,
    }
}

/// A streaming request may receive a whole JSON document even when the gateway
/// omits `application/json`. `None` means either "not JSON" or "not complete yet";
/// a parsed document is safe to commit unless it is a semantic failure envelope.
fn inspect_responses_json_document(buffer: &str) -> Option<Result<(), ProxyError>> {
    let trimmed = buffer.trim();
    if !matches!(trimmed.as_bytes().first(), Some(b'{') | Some(b'[')) {
        return None;
    }
    let _: Value = serde_json::from_str(trimmed).ok()?;
    if let Some(message) = responses_error_envelope_message(trimmed.as_bytes()) {
        return Some(Err(ProxyError::TransformError(format!(
            "Responses upstream returned a 2xx failure: {message}"
        ))));
    }
    Some(Ok(()))
}

/// Inspect one complete Responses SSE block while the response is still inside
/// the retry loop. `None` means the event is lifecycle-only and priming should
/// continue; `Some(Ok(()))` means it is safe to commit/replay the stream.
fn inspect_responses_start_event(block: &str) -> Option<Result<(), ProxyError>> {
    let mut named_event = None;
    let mut data_lines = Vec::new();
    for line in block.lines() {
        if let Some(event) = crate::proxy::sse::strip_sse_field(line, "event") {
            named_event = Some(event.trim().to_string());
        } else if let Some(data) = crate::proxy::sse::strip_sse_field(line, "data") {
            data_lines.push(data);
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    let value: Value = match serde_json::from_str(&data_lines.join("\n")) {
        Ok(value) => value,
        Err(_) => return None,
    };
    let event = named_event
        .as_deref()
        .filter(|event| !event.is_empty())
        .or_else(|| value.get("type").and_then(Value::as_str))
        .unwrap_or("");

    let response = value.get("response").unwrap_or(&value);
    if matches!(
        response.get("status").and_then(Value::as_str),
        Some("failed" | "cancelled")
    ) || response.get("error").is_some_and(|error| !error.is_null())
    {
        let error = response.get("error").unwrap_or(response);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or("Responses upstream failed before output");
        let error_type = error
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| error.get("code").and_then(Value::as_str))
            .or_else(|| response.get("status").and_then(Value::as_str))
            .unwrap_or("upstream_error");
        return Some(Err(ProxyError::TransformError(format!(
            "Responses upstream {error_type}: {message}"
        ))));
    }

    match event {
        "response.failed" | "error" => {
            let error = response.get("error").unwrap_or(response);
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.as_str())
                .unwrap_or("Responses upstream emitted an error before output");
            let error_type = error
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| error.get("code").and_then(Value::as_str))
                .unwrap_or("upstream_error");
            Some(Err(ProxyError::TransformError(format!(
                "Responses upstream {error_type}: {message}"
            ))))
        }
        "response.created" | "response.in_progress" | "response.queued" => None,
        "" => None,
        // Productive output, incomplete, and completed terminals are all safe to
        // expose. Mid-stream failures after this point are surfaced by the converter
        // but intentionally do not switch providers.
        _ => Some(Ok(())),
    }
}

/// Rewrite Codex's `/responses` (and variants) to Anthropic's `/v1/messages`, preserving the query.
fn rewrite_codex_responses_endpoint_to_anthropic(endpoint: &str) -> (String, Option<String>) {
    let (_path, query) = split_endpoint_and_query(endpoint);
    let passthrough_query = query.map(ToString::to_string);
    let target_path = "/v1/messages";
    let rewritten = match passthrough_query.as_deref() {
        Some(query) if !query.is_empty() => format!("{target_path}?{query}"),
        _ => target_path.to_string(),
    };

    (rewritten, passthrough_query)
}





/// Providers whose errors can come from an Anthropic Messages endpoint and therefore
/// need the thinking signature / budget rectifiers. GitHub Copilot passes Anthropic
/// models through to its native `/v1/messages`, where history replayed from another
/// provider or from the Responses bridge fails signature validation; errors from its
/// Chat / Responses bridges never match the rectifier keywords.
fn uses_anthropic_rectifiers(provider_type: Option<ProviderType>) -> bool {
    matches!(
        provider_type,
        Some(ProviderType::Claude | ProviderType::ClaudeAuth)
    )
}



fn rewrite_claude_transform_endpoint(
    endpoint: &str,
    api_format: &str,
    is_copilot: bool,
    body: &Value,
) -> (String, Option<String>) {
    let (path, query) = split_endpoint_and_query(endpoint);
    let passthrough_query = if is_claude_messages_path(path) {
        strip_beta_query(query)
    } else {
        query.map(ToString::to_string)
    };

    if !is_claude_messages_path(path) {
        return (endpoint.to_string(), passthrough_query);
    }

    if api_format == "gemini_native" {
        let model =
            super::providers::transform_gemini::extract_gemini_model(body).unwrap_or("unknown");
        // Accept both bare ids (`gemini-2.5-pro`) and the resource-name
        // form (`models/gemini-2.5-pro`) that Gemini SDKs emit. See
        // `normalize_gemini_model_id` for rationale.
        let model = super::gemini_url::normalize_gemini_model_id(model);
        let is_stream = body
            .get("stream")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let target_path = if is_stream {
            format!("/v1beta/models/{model}:streamGenerateContent")
        } else {
            format!("/v1beta/models/{model}:generateContent")
        };

        let rewritten_query = merge_query_params(
            passthrough_query.as_deref(),
            if is_stream { Some("alt=sse") } else { None },
        );

        let rewritten = match rewritten_query.as_deref() {
            Some(query) if !query.is_empty() => format!("{target_path}?{query}"),
            _ => target_path,
        };

        return (rewritten, rewritten_query);
    }

    let target_path = if is_copilot && api_format == "openai_responses" {
        "/v1/responses"
    } else if is_copilot {
        "/chat/completions"
    } else if api_format == "openai_responses" {
        "/v1/responses"
    } else {
        "/v1/chat/completions"
    };

    let rewritten = match passthrough_query.as_deref() {
        Some(query) if !query.is_empty() => format!("{target_path}?{query}"),
        _ => target_path.to_string(),
    };

    (rewritten, passthrough_query)
}

fn merge_query_params(base_query: Option<&str>, extra_param: Option<&str>) -> Option<String> {
    let mut params: Vec<String> = base_query
        .into_iter()
        .flat_map(|query| query.split('&'))
        .filter(|pair| !pair.is_empty())
        .filter(|pair| !pair.starts_with("alt="))
        .map(ToString::to_string)
        .collect();

    if let Some(extra_param) = extra_param {
        params.push(extra_param.to_string());
    }

    if params.is_empty() {
        None
    } else {
        Some(params.join("&"))
    }
}

fn append_query_to_full_url(base_url: &str, query: Option<&str>) -> String {
    match query {
        Some(query) if !query.is_empty() => {
            if base_url.contains('?') {
                format!("{base_url}&{query}")
            } else {
                format!("{base_url}?{query}")
            }
        }
        _ => base_url.to_string(),
    }
}

fn should_preserve_exact_header_case(
    adapter_name: &str,
    resolved_claude_api_format: Option<&str>,
) -> bool {
    if matches!(adapter_name, "Codex" | "Gemini") {
        return false;
    }

    matches!(resolved_claude_api_format, None | Some("anthropic"))
}

fn is_streaming_request(endpoint: &str, body: &Value, headers: &axum::http::HeaderMap) -> bool {
    if body
        .get("stream")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return true;
    }

    if endpoint.contains("streamGenerateContent") || endpoint.contains("alt=sse") {
        return true;
    }

    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|accept| accept.contains("text/event-stream"))
        .unwrap_or(false)
}

fn map_reqwest_send_error(error: reqwest::Error) -> ProxyError {
    let is_timeout = error.is_timeout();
    let is_connect = error.is_connect();
    let detail = super::error::describe_reqwest_error(error);
    if is_timeout {
        ProxyError::Timeout(format!("上游请求超时: {detail}"))
    } else if is_connect {
        ProxyError::ForwardFailed(format!("上游连接失败: {detail}"))
    } else {
        ProxyError::ForwardFailed(format!("上游请求发送失败: {detail}"))
    }
}

fn summarize_text_for_log(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized.trim();

    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let truncated: String = trimmed.chars().take(max_chars).collect();
    let truncated = truncated.trim_end();
    format!("{truncated}...")
}

fn apply_local_proxy_body_overrides(
    body: &mut Value,
    overrides: &LocalProxyRequestOverrides,
) -> bool {
    let Some(override_body) = overrides.body.as_ref() else {
        return false;
    };

    if !override_body.is_object() {
        log::warn!("[LocalProxyOverrides] Ignoring body override because it is not an object");
        return false;
    }

    merge_json_override(body, override_body)
}

fn merge_json_override(target: &mut Value, patch: &Value) -> bool {
    merge_json_override_inner(target, patch, true)
}

fn merge_json_override_inner(target: &mut Value, patch: &Value, is_top_level: bool) -> bool {
    match (target, patch) {
        (Value::Object(target_map), Value::Object(patch_map)) => {
            let mut changed = false;
            for (key, patch_value) in patch_map {
                if is_top_level && key == "stream" {
                    log::warn!(
                        "[LocalProxyOverrides] Ignoring body override for protected field: stream"
                    );
                    continue;
                }
                match target_map.get_mut(key) {
                    Some(target_value) => {
                        changed |= merge_json_override_inner(target_value, patch_value, false);
                    }
                    None => {
                        target_map.insert(key.clone(), patch_value.clone());
                        changed = true;
                    }
                }
            }
            changed
        }
        (target_value, patch_value) => {
            if target_value == patch_value {
                false
            } else {
                *target_value = patch_value.clone();
                true
            }
        }
    }
}

fn apply_local_proxy_header_overrides(
    headers: &mut http::HeaderMap,
    overrides: Option<&LocalProxyRequestOverrides>,
    is_copilot: bool,
) {
    if is_copilot {
        return;
    }

    let Some(header_overrides) = overrides.map(|overrides| &overrides.headers) else {
        return;
    };

    for (raw_name, raw_value) in header_overrides {
        let header_name = raw_name.trim().to_ascii_lowercase();
        if header_name.is_empty() {
            log::warn!("[LocalProxyOverrides] Ignoring header override with empty name");
            continue;
        }

        let Ok(name) = http::HeaderName::from_bytes(header_name.as_bytes()) else {
            log::warn!("[LocalProxyOverrides] Ignoring invalid header override name: {raw_name}");
            continue;
        };

        if is_protected_local_proxy_override_header(&name) {
            log::debug!(
                "[LocalProxyOverrides] Ignoring protected header override: {}",
                name.as_str()
            );
            continue;
        }

        let Ok(value) = http::HeaderValue::from_str(raw_value) else {
            log::warn!(
                "[LocalProxyOverrides] Ignoring invalid header override value for {}",
                name.as_str()
            );
            continue;
        };

        headers.insert(name, value);
    }
}

fn is_protected_local_proxy_override_header(name: &http::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "te"
            | "trailer"
            | "upgrade"
            | "accept-encoding"
            | "content-type"
            | "authorization"
            | "x-api-key"
            | "x-goog-api-key"
            | "chatgpt-account-id"
            | "session_id"
            | "x-client-request-id"
            | "x-codex-window-id"
            | "x-forwarded-host"
            | "x-forwarded-port"
            | "x-forwarded-proto"
            | "forwarded"
            | "cf-connecting-ip"
            | "cf-ipcountry"
            | "cf-ray"
            | "cf-visitor"
            | "true-client-ip"
            | "fastly-client-ip"
            | "x-azure-clientip"
            | "x-azure-fdid"
            | "x-azure-ref"
            | "akamai-origin-hop"
            | "x-akamai-config-log-detail"
            | "x-request-id"
            | "x-correlation-id"
            | "x-trace-id"
            | "x-amzn-trace-id"
            | "x-b3-traceid"
            | "x-b3-spanid"
            | "x-b3-parentspanid"
            | "x-b3-sampled"
            | "traceparent"
            | "tracestate"
    )
}

fn prepare_upstream_request_body(request_body: Value) -> Value {
    canonicalize_value(filter_private_params_with_whitelist(request_body, &[]))
}

fn log_prompt_cache_trace(
    app_type: &AppType,
    provider: &Provider,
    endpoint: &str,
    api_format: Option<&str>,
    body: &Value,
    session_client_provided: bool,
) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    let prompt_cache_key = body
        .get("prompt_cache_key")
        .and_then(|value| value.as_str())
        .map(|key| format!("present(len={})", key.len()))
        .unwrap_or_else(|| "absent".to_string());
    let store = body
        .get("store")
        .map(value_for_log)
        .unwrap_or_else(|| "absent".to_string());
    let stream = body
        .get("stream")
        .map(value_for_log)
        .unwrap_or_else(|| "absent".to_string());
    let cache_controls = cache_control_summary(body);

    log::debug!(
        "[CacheTrace] app={}, provider={}, endpoint={}, api_format={}, session_client_provided={}, prompt_cache_key={}, store={}, stream={}, instructions_hash={}, system_hash={}, tools_hash={}, input_hash={}, messages_hash={}, include_hash={}, cache_controls={}, body_hash={}",
        app_type.as_str(),
        provider.id,
        // Gemini 的 endpoint 带 ?key=<API_KEY>；脱敏剥掉 query 再落盘。
        crate::redact_url_for_log(endpoint),
        api_format.unwrap_or("native"),
        session_client_provided,
        prompt_cache_key,
        store,
        stream,
        short_value_hash(body.get("instructions")),
        short_value_hash(body.get("system")),
        short_value_hash(body.get("tools")),
        short_value_hash(body.get("input")),
        short_value_hash(body.get("messages")),
        short_value_hash(body.get("include")),
        cache_controls,
        short_value_hash(Some(body)),
    );
}

fn cache_control_summary(value: &Value) -> String {
    fn walk(value: &Value, count: &mut usize, ttls: &mut std::collections::BTreeSet<String>) {
        match value {
            Value::Object(object) => {
                if let Some(cache_control) = object.get("cache_control") {
                    *count += 1;
                    let ttl = cache_control
                        .get("ttl")
                        .and_then(Value::as_str)
                        .unwrap_or("default");
                    ttls.insert(ttl.to_string());
                }
                for child in object.values() {
                    walk(child, count, ttls);
                }
            }
            Value::Array(items) => {
                for child in items {
                    walk(child, count, ttls);
                }
            }
            _ => {}
        }
    }

    let mut count = 0;
    let mut ttls = std::collections::BTreeSet::new();
    walk(value, &mut count, &mut ttls);
    format!(
        "count={count},ttls={}",
        if ttls.is_empty() {
            "none".to_string()
        } else {
            ttls.into_iter().collect::<Vec<_>>().join("|")
        }
    )
}

fn value_for_log(value: &Value) -> String {
    match value {
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Null => "null".to_string(),
        Value::Array(values) => format!("array(len={})", values.len()),
        Value::Object(values) => format!("object(len={})", values.len()),
    }
}

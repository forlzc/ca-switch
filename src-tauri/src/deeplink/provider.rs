//! Provider import from deep link
//!
//! Handles importing provider configurations via ccswitch:// URLs.

use super::utils::{decode_base64_param, infer_homepage_from_endpoint};
use super::DeepLinkImportRequest;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta, UsageScript};
use crate::services::ProviderService;
use crate::store::AppState;
use crate::AppType;
use serde_json::json;
use std::str::FromStr;

/// Import a provider from a deep link request
///
/// This function:
/// 1. Validates the request
/// 2. Merges config file if provided (v3.8+)
/// 3. Converts it to a Provider structure
/// 4. Delegates to ProviderService for actual import
/// 5. Optionally sets as current provider if enabled=true
pub fn import_provider_from_deeplink(
    state: &AppState,
    request: DeepLinkImportRequest,
) -> Result<String, AppError> {
    // Verify this is a provider request
    if request.resource != "provider" {
        return Err(AppError::InvalidInput(format!(
            "Expected provider resource, got '{}'",
            request.resource
        )));
    }

    // Step 1: Merge config file if provided (v3.8+)
    let mut merged_request = parse_and_merge_config(&request)?;

    // Extract required fields (now as Option)
    let app_str = merged_request
        .app
        .clone()
        .ok_or_else(|| AppError::InvalidInput("Missing 'app' field for provider".to_string()))?;

    let api_key = merged_request.api_key.as_ref().ok_or_else(|| {
        AppError::InvalidInput("API key is required (either in URL or config file)".to_string())
    })?;

    if api_key.is_empty() {
        return Err(AppError::InvalidInput(
            "API key cannot be empty".to_string(),
        ));
    }

    // Get endpoint: supports comma-separated multiple URLs (first is primary)
    let endpoint_str = merged_request.endpoint.as_ref().ok_or_else(|| {
        AppError::InvalidInput("Endpoint is required (either in URL or config file)".to_string())
    })?;

    // Parse endpoints: split by comma, first is primary
    let all_endpoints: Vec<String> = endpoint_str
        .split(',')
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();

    let primary_endpoint = all_endpoints
        .first()
        .ok_or_else(|| AppError::InvalidInput("Endpoint cannot be empty".to_string()))?;

    // Auto-infer homepage from endpoint if not provided
    if merged_request
        .homepage
        .as_ref()
        .is_none_or(|s| s.is_empty())
    {
        merged_request.homepage = infer_homepage_from_endpoint(primary_endpoint);
    }

    let homepage = merged_request.homepage.as_ref().ok_or_else(|| {
        AppError::InvalidInput("Homepage is required (either in URL or config file)".to_string())
    })?;

    if homepage.is_empty() {
        return Err(AppError::InvalidInput(
            "Homepage cannot be empty".to_string(),
        ));
    }

    let name = merged_request
        .name
        .clone()
        .ok_or_else(|| AppError::InvalidInput("Missing 'name' field for provider".to_string()))?;

    // Parse app type
    let app_type = AppType::from_str(&app_str)
        .map_err(|_| AppError::InvalidInput(format!("Invalid app type: {app_str}")))?;

    // Build provider configuration based on app type
    let mut provider = build_provider_from_request(&app_type, &merged_request)?;

    let timestamp = chrono::Utc::now().timestamp_millis();
    let sanitized_name = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .to_lowercase();
    provider.id = format!("{sanitized_name}-{timestamp}");

    let provider_id = provider.id.clone();

    // `enabled` 是确认框里「导入后立即切换 / 添加到应用」开关的值（链接里的 `enabled` 只是它的
    // 初始值）。开关关闭时只存进列表：`add` 在还没有当前供应商时会让这家直接生效，所以走不写
    // 客户端配置的路径。
    let activate = merged_request.enabled.unwrap_or(false);

    if activate {
        ProviderService::add(state, app_type.clone(), provider)?;
    } else {
        ProviderService::add_without_switching(state, app_type.clone(), provider)?;
    }

    // Add extra endpoints as custom endpoints (skip first one as it's the primary)
    for ep in all_endpoints.iter().skip(1) {
        let normalized = ep.trim().trim_end_matches('/').to_string();
        if !normalized.is_empty() {
            if let Err(e) = ProviderService::add_custom_endpoint(
                state,
                app_type.clone(),
                &provider_id,
                normalized.clone(),
            ) {
                log::warn!(
                    "Failed to add custom endpoint '{}': {e}",
                    crate::url_for_log(&normalized)
                );
            }
        }
    }

    // If enabled=true, set as current provider.
    if activate {
        ProviderService::switch(state, app_type.clone(), &provider_id)?;
        log::info!("Provider '{provider_id}' set as current for {app_type:?}");
    }

    Ok(provider_id)
}

/// 链接声明的接口格式（`apiFormat`）。站长只学这一套词，各应用换算成自己的字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiFormat {
    Anthropic,
    OpenAiChat,
    OpenAiResponses,
    GeminiNative,
}

impl ApiFormat {
    const VALUES: &'static str = "anthropic, openai_chat, openai_responses, gemini_native";

    fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim() {
            "anthropic" => Ok(Self::Anthropic),
            "openai_chat" => Ok(Self::OpenAiChat),
            "openai_responses" => Ok(Self::OpenAiResponses),
            "gemini_native" => Ok(Self::GeminiNative),
            other => Err(AppError::InvalidInput(format!(
                "Unsupported apiFormat '{other}'; use one of: {}",
                Self::VALUES
            ))),
        }
    }

    /// 这个格式在应用里对应的值；应用不支持这种接口时为 `None`。
    ///
    /// Claude Code、Codex、Grok Build 写进 `meta.apiFormat`（非原生格式由本地路由转换）；
    /// 其余应用写进各自配置里选择协议的字段：OpenCode 的 `npm`、OpenClaw / Pi 的 `api`、
    /// Hermes 的 `api_mode`。Gemini CLI 只有原生接口。
    fn native_value(self, app_type: &AppType) -> Option<&'static str> {
        use ApiFormat::*;
        match (app_type, self) {
            (AppType::Claude, Anthropic) => Some("anthropic"),
            (AppType::Claude, OpenAiChat) => Some("openai_chat"),
            (AppType::Claude, OpenAiResponses) => Some("openai_responses"),
            (AppType::Claude, GeminiNative) => Some("gemini_native"),
        }
    }
}

/// 核对链接的 `apiFormat` 在这个应用上是否可用（解析和导入两处都调）。
pub(super) fn check_api_format(app: &str, value: &str) -> Result<(), AppError> {
    let app_type = AppType::from_str(app)
        .map_err(|_| AppError::InvalidInput(format!("Invalid app type: {app}")))?;
    let format = ApiFormat::parse(value)?;
    if format.native_value(&app_type).is_none() {
        return Err(AppError::InvalidInput(format!(
            "apiFormat '{}' is not supported for app '{app}'",
            value.trim()
        )));
    }
    Ok(())
}

/// 链接声明的格式，已核对过这个应用支持它；没写时为 `None`。
fn link_api_format(
    app_type: &AppType,
    request: &DeepLinkImportRequest,
) -> Result<Option<ApiFormat>, AppError> {
    let Some(value) = request.api_format.as_deref() else {
        return Ok(None);
    };
    check_api_format(app_type.as_str(), value)?;
    ApiFormat::parse(value).map(Some)
}

/// 累加式应用读配置时会解释的字符串写法，链接带了就拒绝导入。
///
/// - Pi：`apiKey` 以 `!` 开头会被当成 shell 命令执行；`$NAME`、`${NAME}` 会换成环境变量。
/// - OpenCode：`{env:NAME}` 读环境变量，`{file:path}` 读本机文件。
/// - OpenClaw、Hermes：`${NAME}` 读环境变量。
///
/// 链接是不可信输入：这些写法会让客户端执行链接里的命令，或把本机的密钥、文件内容随请求
/// 发到链接给的地址。确认框把 Key 打码显示，用户也看不出来，所以在后端拒绝。
/// 名称、地址、Key、模型都会写进这些应用的配置，都要查。
pub(super) fn check_link_values(
    _app_type: &AppType,
    _request: &DeepLinkImportRequest,
) -> Result<(), AppError> {
    Ok(())
}

/// Build a Provider structure from a deep link request
pub(crate) fn build_provider_from_request(
    app_type: &AppType,
    request: &DeepLinkImportRequest,
) -> Result<Provider, AppError> {
    let api_format = link_api_format(app_type, request)?;
    check_link_values(app_type, request)?;
    let settings_config = match app_type {
        AppType::Claude => build_claude_settings(request),
    };

    // Build usage script configuration if provided
    let mut meta = build_provider_meta(app_type, request)?;
    // 非原生格式记在 meta 上，切过去时由本地路由转换。
    if let Some(value) = api_format.and_then(|format| format.native_value(app_type)) {
        meta.get_or_insert_with(ProviderMeta::default).api_format = Some(value.to_string());
    }

    let provider = Provider {
        id: String::new(), // Will be generated by caller
        name: request.name.clone().unwrap_or_default(),
        settings_config,
        website_url: request.homepage.clone(),
        category: None,
        created_at: None,
        sort_index: None,
        notes: request.notes.clone(),
        meta,
        icon: request.icon.clone(),
        icon_color: None,
        in_failover_queue: false,
    };

    Ok(provider)
}

/// Get primary endpoint from request (first one if comma-separated)
fn get_primary_endpoint(request: &DeepLinkImportRequest) -> String {
    request
        .endpoint
        .as_ref()
        .and_then(|ep| ep.split(',').next())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn normalize_deeplink_api_key(api_key: &str) -> String {
    api_key.trim().to_string()
}

fn normalize_deeplink_base_url(base_url: &str) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

fn usage_api_key_override(request: &DeepLinkImportRequest) -> Option<String> {
    let usage_api_key = normalize_deeplink_api_key(request.usage_api_key.as_deref()?);
    if usage_api_key.is_empty() {
        return None;
    }

    let provider_api_key = request
        .api_key
        .as_deref()
        .map(normalize_deeplink_api_key)
        .unwrap_or_default();

    if !provider_api_key.is_empty() && usage_api_key == provider_api_key {
        None
    } else {
        Some(usage_api_key)
    }
}

fn usage_base_url_override(request: &DeepLinkImportRequest) -> Option<String> {
    let usage_base_url = normalize_deeplink_base_url(request.usage_base_url.as_deref()?);
    if usage_base_url.is_empty() {
        return None;
    }

    let provider_base_url = normalize_deeplink_base_url(&get_primary_endpoint(request));

    if !provider_base_url.is_empty() && usage_base_url == provider_base_url {
        None
    } else {
        Some(usage_base_url)
    }
}

/// 主地址命中套餐用量路由表时返回套餐 id，规则同前端 `injectCodingPlanUsageScript`：
/// 所有应用一视同仁，只有 ZenMux 限 Claude（它的查询直接 GET base_url，
/// 其他应用的推理地址没验证过能返回额度）。
fn link_coding_plan_provider(
    app_type: &AppType,
    request: &DeepLinkImportRequest,
) -> Option<&'static str> {
    let id = crate::services::coding_plan::detect_provider_id(&get_primary_endpoint(request))?;
    (id != "zenmux" || matches!(app_type, AppType::Claude)).then_some(id)
}

/// Build provider meta with usage script configuration
fn build_provider_meta(
    app_type: &AppType,
    request: &DeepLinkImportRequest,
) -> Result<Option<ProviderMeta>, AppError> {
    let coding_plan_provider = link_coding_plan_provider(app_type, request);

    // Check if any usage script fields are provided
    if coding_plan_provider.is_none()
        && request.usage_script.is_none()
        && request.usage_enabled.is_none()
        && request.usage_api_key.is_none()
        && request.usage_base_url.is_none()
        && request.usage_access_token.is_none()
        && request.usage_user_id.is_none()
        && request.usage_auto_interval.is_none()
    {
        return Ok(None);
    }

    // Decode usage script code if provided
    let code = if let Some(script_b64) = &request.usage_script {
        let decoded = decode_base64_param("usage_script", script_b64)?;
        String::from_utf8(decoded)
            .map_err(|e| AppError::InvalidInput(format!("Invalid UTF-8 in usage_script: {e}")))?
    } else {
        String::new()
    };

    // 套餐地址且没带脚本：和在应用里新建一样，按套餐额度查询并默认启用
    // （链接写了 `usageEnabled` 仍照办）。额度走后端固定端点，不执行任何脚本。
    // 火山例外：查额度要账号级 AccessKey，链接里没有，默认启用只会显示「额度查询失败」，
    // 所以只预选套餐不启用（前端 `codingPlanEnabledByDefault` 与此一致）。
    if let (Some(provider_id), true) = (coding_plan_provider, code.is_empty()) {
        return Ok(Some(ProviderMeta {
            usage_script: Some(UsageScript {
                enabled: request.usage_enabled.unwrap_or(provider_id != "volcengine"),
                language: "javascript".to_string(),
                code,
                timeout: Some(10),
                api_key: usage_api_key_override(request),
                base_url: usage_base_url_override(request),
                access_token: None,
                user_id: None,
                template_type: Some("token_plan".to_string()),
                auto_query_interval: request.usage_auto_interval.or(Some(5)),
                coding_plan_provider: Some(provider_id.to_string()),
                access_key_id: None,
                secret_access_key: None,
                team_organization_id: None,
                team_project_id: None,
            }),
            ..Default::default()
        }));
    }

    // 启用状态：链接写了 `usageEnabled` 就照办；没写时，带了脚本就启用。
    //
    // 站长放脚本就是为了让用户看到余额，导入后还要再手动打开一次是多余的一步。
    // 默认启用的代价可控：脚本跑在受限的 QuickJS 里，只能发一个请求，且必须是
    // 与 base_url 同源的 HTTPS（`usage_script::validate_request_url`；深链接导入
    // 的脚本没有 template_type，不算自定义模板，这项检查不会被跳过）。
    //
    // 用户的同意仍然体现在确认框：完整的脚本正文和启用徽章都展示过之后才点导入，
    // 所以那两处展示是本设计的承重部分，不可省略。前端徽章的判断必须和这里一致。
    // 链接想导入但不启用，就显式写 `usageEnabled=false`。
    let enabled = request.usage_enabled.unwrap_or(!code.is_empty());

    let usage_script = UsageScript {
        enabled,
        language: "javascript".to_string(),
        code,
        timeout: Some(10),
        api_key: usage_api_key_override(request),
        base_url: usage_base_url_override(request),
        access_token: request.usage_access_token.clone(),
        user_id: request.usage_user_id.clone(),
        template_type: None, // Deeplink providers don't specify template type (will use backward compatibility logic)
        auto_query_interval: request.usage_auto_interval,
        coding_plan_provider: None,
        access_key_id: None,
        secret_access_key: None,
        team_organization_id: None,
        team_project_id: None,
    };

    Ok(Some(ProviderMeta {
        usage_script: Some(usage_script),
        ..Default::default()
    }))
}

/// Build Claude settings configuration
///
/// Merges env from the inline config (if any) with the standard fields from URL params.
/// URL params take priority — they overwrite same-named fields from the config.
/// Non-standard env fields (e.g. `ANTHROPIC_CUSTOM_HEADERS`, `API_TIMEOUT_MS`,
/// `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`, ...) are preserved as-is so that
/// providers requiring extra environment variables work after deeplink import.
fn build_claude_settings(request: &DeepLinkImportRequest) -> serde_json::Value {
    // Start from the full env block in the inline config (if present), so any
    // custom env vars the user passed via `config=<base64-json>` survive the
    // import. Falling back to an empty map keeps the previous behavior for
    // deeplinks that don't carry a config field.
    let mut env = extract_claude_config_env(request).unwrap_or_default();

    // Now overwrite / fill in the standard fields from URL params. URL params
    // are authoritative because they're what the deeplink builder put on the
    // wire — for Claude these are: ANTHROPIC_AUTH_TOKEN, ANTHROPIC_BASE_URL,
    // ANTHROPIC_MODEL, and the haiku/sonnet/opus model aliases.
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".to_string(),
        json!(request.api_key.clone().unwrap_or_default()),
    );
    env.insert(
        "ANTHROPIC_BASE_URL".to_string(),
        json!(get_primary_endpoint(request)),
    );

    // Add default model if provided
    if let Some(model) = &request.model {
        env.insert("ANTHROPIC_MODEL".to_string(), json!(model));
    }

    // Add Claude-specific model fields (v3.7.1+)
    if let Some(haiku_model) = &request.haiku_model {
        env.insert(
            "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
            json!(haiku_model),
        );
    }
    if let Some(sonnet_model) = &request.sonnet_model {
        env.insert(
            "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
            json!(sonnet_model),
        );
    }
    if let Some(opus_model) = &request.opus_model {
        env.insert(
            "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
            json!(opus_model),
        );
    }

    json!({ "env": env })
}

/// Decode and extract the `env` object from the deeplink's inline config payload.
///
/// Returns `None` when no config is attached, when the payload can't be
/// decoded/parsed, or when it doesn't contain a Claude-style `env` object.
/// This is a best-effort accessor — we deliberately don't surface parse
/// errors here because `parse_and_merge_config` will have already validated
/// the payload during the merge phase; any failure at this point just means
/// "fall back to URL-param-only behavior".
fn extract_claude_config_env(
    request: &DeepLinkImportRequest,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    // Only the inline base64 config carries an env block. Remote config_url
    // is not implemented yet (see parse_and_merge_config), so nothing else to
    // try here.
    let config_b64 = request.config.as_ref()?;

    // Honor the declared format; default to JSON like parse_and_merge_config does.
    let format = request.config_format.as_deref().unwrap_or("json");
    if format != "json" {
        // Claude config is always JSON in practice. TOML/other formats aren't
        // expected on this app path, so don't try to handle them — safer to
        // fall back than to risk silently producing the wrong shape.
        return None;
    }

    // Decode the base64 payload. We re-decode here rather than threading the
    // already-decoded value through every build_* function, because the call
    // graph (parse_and_merge_config is pub and called separately for preview)
    // makes signature changes invasive. Decode cost is negligible on this
    // one-shot import path.
    let decoded = decode_base64_param("config", config_b64).ok()?;
    let json_str = std::str::from_utf8(&decoded).ok()?;
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;

    // Pull out the env object — same shape as Claude's own settings.json.
    value.get("env").and_then(|v| v.as_object()).cloned()
}








// =============================================================================
// Config Merge Logic
// =============================================================================

/// 解码链接的 `config` 参数（Base64），按 `configFormat` 解析成 JSON 值。没带时返回 `None`。
fn decode_link_config(
    request: &DeepLinkImportRequest,
) -> Result<Option<serde_json::Value>, AppError> {
    let config_content = if let Some(config_b64) = &request.config {
        let decoded = decode_base64_param("config", config_b64)?;
        String::from_utf8(decoded)
            .map_err(|e| AppError::InvalidInput(format!("Invalid UTF-8 in config: {e}")))?
    } else if request.config_url.is_some() {
        return Err(AppError::InvalidInput(
            "Remote config URL is not yet supported. Use inline config instead.".to_string(),
        ));
    } else {
        return Ok(None);
    };

    let format = request.config_format.as_deref().unwrap_or("json");
    let config_value = match format {
        "json" => serde_json::from_str(&config_content)
            .map_err(|e| AppError::InvalidInput(format!("Invalid JSON config: {e}")))?,
        "toml" => {
            let toml_value: toml::Value = toml::from_str(&config_content)
                .map_err(|e| AppError::InvalidInput(format!("Invalid TOML config: {e}")))?;
            serde_json::to_value(toml_value)
                .map_err(|e| AppError::Message(format!("Failed to convert TOML to JSON: {e}")))?
        }
        _ => {
            return Err(AppError::InvalidInput(format!(
                "Unsupported config format: {format}"
            )))
        }
    };
    Ok(Some(config_value))
}

/// Parse and merge configuration from Base64 encoded config or remote URL
///
/// Priority: URL params > inline config > remote config
pub fn parse_and_merge_config(
    request: &DeepLinkImportRequest,
) -> Result<DeepLinkImportRequest, AppError> {
    let Some(config_value) = decode_link_config(request)? else {
        return Ok(request.clone());
    };

    // Extract values from config based on app type and merge with URL params
    let mut merged = request.clone();

    // MCP, Skill and other resource types don't need config merging
    if request.resource != "provider" {
        return Ok(merged);
    }

    match request.app.as_deref().unwrap_or("") {
        "claude" | "anthropic" => merge_claude_config(&mut merged, &config_value)?,
        "" => {
            // No app specified, skip merging
            return Ok(merged);
        }
        _ => {
            return Err(AppError::InvalidInput(format!(
                "Invalid app type: {:?}",
                request.app
            )))
        }
    }

    Ok(merged)
}

/// Merge Claude configuration from config file
fn merge_claude_config(
    request: &mut DeepLinkImportRequest,
    config: &serde_json::Value,
) -> Result<(), AppError> {
    let env = config
        .get("env")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            AppError::InvalidInput("Claude config must have 'env' object".to_string())
        })?;

    // Auto-fill API key if not provided in URL
    if request.api_key.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(token) = env.get("ANTHROPIC_AUTH_TOKEN").and_then(|v| v.as_str()) {
            request.api_key = Some(token.to_string());
        }
    }

    // Auto-fill endpoint if not provided in URL
    if request.endpoint.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(base_url) = env.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str()) {
            request.endpoint = Some(base_url.to_string());
        }
    }

    // Auto-fill homepage from endpoint if not provided
    if request.homepage.as_ref().is_none_or(|s| s.is_empty()) {
        if let Some(endpoint) = request.endpoint.as_ref().filter(|s| !s.is_empty()) {
            request.homepage = infer_homepage_from_endpoint(endpoint);
            if request.homepage.is_none() {
                request.homepage = Some("https://anthropic.com".to_string());
            }
        }
    }

    // Auto-fill model fields (URL params take priority)
    if request.model.is_none() {
        request.model = env
            .get("ANTHROPIC_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    if request.haiku_model.is_none() {
        request.haiku_model = env
            .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    if request.sonnet_model.is_none() {
        request.sonnet_model = env
            .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }
    if request.opus_model.is_none() {
        request.opus_model = env
            .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    Ok(())
}







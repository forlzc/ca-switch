//! 官方订阅额度查询服务
//!
//! 读取 CLI 工具的已有 OAuth 凭据，查询官方订阅额度。
//! 第一层：仅读取凭据，不实现登录/刷新。

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use std::collections::HashSet;

use crate::config;

// ── 数据类型 ──────────────────────────────────────────────

/// 凭据状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Valid,
    Expired,
    /// 访问令牌过期，但刷新令牌还能用：客户端下次运行时自己会换新的，不用重新登录。
    RefreshPending,
    NotFound,
    ParseError,
}

/// 单个限速窗口（如 5小时会话、7天周期）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaTier {
    /// 窗口标识：five_hour, seven_day, seven_day_fable, seven_day_opus 等
    pub name: String,
    /// 使用百分比 0–100
    pub utilization: f64,
    /// ISO 8601 重置时间
    pub resets_at: Option<String>,
    /// ZenMux: 已用额度（USD）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_value_usd: Option<f64>,
    /// ZenMux: 窗口上限（USD）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_value_usd: Option<f64>,
}

/// 超额使用信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
    pub currency: Option<String>,
}

/// ChatGPT 订阅存下的限额重置次数（Codex「存下重置、需要时再用」）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCredits {
    /// 每一次可用重置的到期时间（ISO 8601），先到期的在前，不过期的是 None 排最后；
    /// 长度就是可用次数（只收 available 且查询时还没过期的）
    pub expires_at: Vec<Option<String>>,
}

/// 订阅额度查询结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionQuota {
    pub tool: String,
    pub credential_status: CredentialStatus,
    pub credential_message: Option<String>,
    pub success: bool,
    pub tiers: Vec<QuotaTier>,
    pub extra_usage: Option<ExtraUsage>,
    /// 只有 ChatGPT 订阅（codex / codex_oauth）有；没查到时为 None，不影响额度本身
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_credits: Option<ResetCredits>,
    /// ChatGPT 订阅买的 Codex Credits 余额（额度用完后才扣）；没有、不限量或为 0 时为 None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits_balance: Option<f64>,
    pub error: Option<String>,
    pub queried_at: Option<i64>,
}

impl SubscriptionQuota {
    pub(crate) fn not_found(tool: &str) -> Self {
        Self {
            tool: tool.to_string(),
            credential_status: CredentialStatus::NotFound,
            credential_message: None,
            success: false,
            tiers: vec![],
            extra_usage: None,
            reset_credits: None,
            credits_balance: None,
            error: None,
            queried_at: None,
        }
    }

    pub(crate) fn error(tool: &str, status: CredentialStatus, message: String) -> Self {
        Self {
            tool: tool.to_string(),
            credential_status: status,
            credential_message: Some(message.clone()),
            success: false,
            tiers: vec![],
            extra_usage: None,
            reset_credits: None,
            credits_balance: None,
            error: Some(message),
            queried_at: Some(now_millis()),
        }
    }
}

// ── Claude 凭据读取 ──────────────────────────────────────

/// Claude OAuth 凭据文件中的嵌套结构
#[derive(Deserialize)]
struct ClaudeOAuthEntry {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<serde_json::Value>,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "refreshTokenExpiresAt")]
    refresh_token_expires_at: Option<serde_json::Value>,
}

/// 读取 Claude OAuth 凭据
///
/// 按优先级尝试以下来源：
/// 1. macOS Keychain（先匹配当前用户账户，再兼容只按 service 查询）
/// 2. 凭据文件 ~/.claude/.credentials.json
///
/// JSON 格式（两种 key 都兼容）：
/// {"claudeAiOauth": {"accessToken": "...", "expiresAt": ...}}
/// {"claude.ai_oauth": {"accessToken": "...", "expiresAt": ...}}
fn read_claude_credentials() -> (Option<String>, CredentialStatus, Option<String>) {
    // 来源 1: macOS Keychain
    #[cfg(target_os = "macos")]
    {
        if let Some(result) = read_claude_credentials_from_keychain() {
            return result;
        }
    }

    // 来源 2: 凭据文件
    read_claude_credentials_from_file()
}

/// 从 macOS Keychain 读取 Claude 凭据
#[cfg(target_os = "macos")]
fn read_claude_credentials_from_keychain(
) -> Option<(Option<String>, CredentialStatus, Option<String>)> {
    let account = std::env::var("USER").ok().filter(|user| !user.is_empty());
    read_claude_credentials_from_keychain_with(account.as_deref(), |command| command.output())
}

#[cfg(target_os = "macos")]
fn read_claude_credentials_from_keychain_with(
    account: Option<&str>,
    run: impl FnMut(&mut std::process::Command) -> std::io::Result<std::process::Output>,
) -> Option<(Option<String>, CredentialStatus, Option<String>)> {
    claude_keychain_json_with(account, run).map(|json| parse_claude_credentials_json(&json))
}

/// 钥匙串里带 Claude 登录的那条凭据（JSON）；读不了钥匙串或没有时为 `None`（交给文件）。
#[cfg(target_os = "macos")]
fn claude_keychain_json_with(
    account: Option<&str>,
    mut run: impl FnMut(&mut std::process::Command) -> std::io::Result<std::process::Output>,
) -> Option<String> {
    for account in account.into_iter().map(Some).chain(std::iter::once(None)) {
        let mut command = std::process::Command::new("security");
        command.args(["find-generic-password", "-s", "Claude Code-credentials"]);
        if let Some(account) = account {
            command.args(["-a", account]);
        }
        command.arg("-w");

        let output = match run(&mut command) {
            Ok(output) if output.status.success() => output,
            Ok(output) if output.status.code() == Some(SECURITY_ITEM_NOT_FOUND) => continue,
            _ => return None,
        };
        let Ok(json_str) = String::from_utf8(output.stdout) else {
            continue;
        };
        let json = json_str.trim();
        // 同名条目可能只存了 mcpOAuth；没有 Claude token 时继续找，最后回退到文件。
        // 已过期的 Claude token 仍是该账户的凭据，保留其过期/待刷新状态。
        if parse_claude_credentials_json(json).0.is_some() {
            return Some(json.to_string());
        }
    }
    None
}



/// 从文件读取 Claude 凭据
fn read_claude_credentials_from_file() -> (Option<String>, CredentialStatus, Option<String>) {
    let cred_path = config::get_claude_config_dir().join(".credentials.json");

    if !cred_path.exists() {
        return (None, CredentialStatus::NotFound, None);
    }

    let content = match std::fs::read_to_string(&cred_path) {
        Ok(c) => c,
        Err(e) => {
            return (
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to read credentials file: {e}")),
            );
        }
    };

    parse_claude_credentials_json(&content)
}

/// 解析 Claude 凭据 JSON（Keychain 和文件共用）
fn parse_claude_credentials_json(
    content: &str,
) -> (Option<String>, CredentialStatus, Option<String>) {
    let parsed: serde_json::Value = match serde_json::from_str(content) {
        Ok(v) => v,
        Err(e) => {
            return (
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to parse credentials JSON: {e}")),
            );
        }
    };

    // 兼容两种 key 名
    let entry_value = parsed
        .get("claudeAiOauth")
        .or_else(|| parsed.get("claude.ai_oauth"));

    let entry_value = match entry_value {
        Some(v) => v,
        None => {
            return (
                None,
                CredentialStatus::ParseError,
                Some("No OAuth entry found in credentials".to_string()),
            );
        }
    };

    let entry: ClaudeOAuthEntry = match serde_json::from_value(entry_value.clone()) {
        Ok(e) => e,
        Err(e) => {
            return (
                None,
                CredentialStatus::ParseError,
                Some(format!("Failed to parse OAuth entry: {e}")),
            );
        }
    };

    let access_token = match entry.access_token {
        Some(t) if !t.is_empty() => t,
        _ => {
            return (
                None,
                CredentialStatus::ParseError,
                Some("accessToken is empty or missing".to_string()),
            );
        }
    };

    // 检查 token 是否过期
    if let Some(expires_at) = entry.expires_at {
        if is_token_expired(&expires_at) {
            // 访问令牌几小时一换，Claude Code 下次运行时用刷新令牌换新的；
            // 刷新令牌还在就不算登录过期。
            let refreshable = entry.refresh_token.is_some_and(|t| !t.is_empty())
                && !entry
                    .refresh_token_expires_at
                    .as_ref()
                    .is_some_and(is_token_expired);
            return if refreshable {
                (
                    Some(access_token),
                    CredentialStatus::RefreshPending,
                    Some(
                        "Access token has expired; Claude Code refreshes it the next time it runs"
                            .to_string(),
                    ),
                )
            } else {
                (
                    Some(access_token),
                    CredentialStatus::Expired,
                    Some("OAuth token has expired".to_string()),
                )
            };
        }
    }

    (Some(access_token), CredentialStatus::Valid, None)
}

/// 判断 token 是否过期，兼容 Unix 时间戳（秒/毫秒）和 ISO 字符串
fn is_token_expired(expires_at: &serde_json::Value) -> bool {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    match expires_at {
        serde_json::Value::Number(n) => {
            if let Some(ts) = n.as_u64() {
                // 区分秒和毫秒（毫秒级时间戳大于 1e12）
                let ts_secs = if ts > 1_000_000_000_000 {
                    ts / 1000
                } else {
                    ts
                };
                ts_secs < now_secs
            } else {
                false
            }
        }
        serde_json::Value::String(s) => {
            // 尝试解析 ISO 8601 格式
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                (dt.timestamp() as u64) < now_secs
            } else if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
            {
                (dt.and_utc().timestamp() as u64) < now_secs
            } else {
                false // 无法解析时不视为过期
            }
        }
        _ => false,
    }
}

// ── Claude API 查询 ──────────────────────────────────────

/// Claude OAuth 用量 API 响应中的单个窗口
#[derive(Deserialize)]
struct ApiUsageWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

/// `limits[]` 中的窗口使用 `percent`，而非旧顶层窗口的 `utilization`。
#[derive(Deserialize)]
struct ApiScopedUsageWindow {
    percent: f64,
    resets_at: Option<String>,
}

/// Claude OAuth 用量 API 响应中的超额用量
#[derive(Deserialize)]
struct ApiExtraUsage {
    is_enabled: Option<bool>,
    monthly_limit: Option<f64>,
    used_credits: Option<f64>,
    utilization: Option<f64>,
    currency: Option<String>,
}

/// 已知的 Claude 用量窗口名称；未知的旧格式窗口仍保留原名称。
pub const TIER_FIVE_HOUR: &str = "five_hour";
pub const TIER_SEVEN_DAY: &str = "seven_day";
/// 内部统一名称：Fable 实际由 `limits[].scope.model` 标识。
pub const TIER_SEVEN_DAY_FABLE: &str = "seven_day_fable";
pub const TIER_SEVEN_DAY_OPUS: &str = "seven_day_opus";
pub const TIER_SEVEN_DAY_SONNET: &str = "seven_day_sonnet";

/// Coding Plan（Kimi / MiniMax）的周窗口 tier 名。与 `coding_plan::query_*`
/// 写入、tray 渲染、commands::provider 扁平化三处共用同一标识。
pub const TIER_WEEKLY_LIMIT: &str = "weekly_limit";

/// 月窗口 tier 名。火山方舟 Agent Plan / Coding Plan 有 5h / 周 / 月 三个展示
/// 窗口（Kimi / MiniMax 只有 5h + 周），月窗口共用此标识；前端 `TIER_I18N_KEYS`
/// 映射到 `subscription.monthly`。
pub const TIER_MONTHLY: &str = "monthly";

/// Codex 免费方案的 30 天（月）滚动窗口 tier 名。付费方案的次要窗口是 7 天
/// (`seven_day`)，免费方案则是 30 天。由 `window_seconds_to_tier_name` 产出、
/// tray 的月分组渲染、前端 `TIER_I18N_KEYS` 映射到 `subscription.thirtyDay`
/// 三处共用同一标识。见 #3651。
pub const TIER_THIRTY_DAY: &str = "30_day";

/// Grok credit 额度窗口的兜底 tier 名。Grok 账单接口只返回一个 credit 用量
/// 窗口，`subscription_grok::tier_name_for_reset` 按重置距离优先映射到
/// `weekly_limit` / `monthly`，两者都不匹配时用此标识；前端 `TIER_I18N_KEYS`
/// 映射到 `subscription.credits`，tray 归入 "c" 分组。
pub const TIER_CREDITS: &str = "credits";

/// Gemini 用量分组名称（按模型而非时间窗口）。`classify_gemini_model` 输出。
pub const TIER_GEMINI_PRO: &str = "gemini_pro";
pub const TIER_GEMINI_FLASH: &str = "gemini_flash";
pub const TIER_GEMINI_FLASH_LITE: &str = "gemini_flash_lite";

const KNOWN_TIERS: &[&str] = &[
    TIER_FIVE_HOUR,
    TIER_SEVEN_DAY,
    TIER_SEVEN_DAY_FABLE,
    TIER_SEVEN_DAY_OPUS,
    TIER_SEVEN_DAY_SONNET,
];

/// 查询 Claude 官方订阅额度
///
/// 瞬时传输失败（网络/超时/读体中断）返回 `Err`（前端 reject → retry + 保留上次
/// 成功值）；确定性失败（鉴权/非 2xx/响应体非法 JSON）返回 `Ok(success:false)`。
/// codex/gemini 两个查询函数遵守同一约定。
async fn query_claude_quota(access_token: &str) -> Result<SubscriptionQuota, String> {
    let client = crate::proxy::http_client::get();

    let resp = client
        .get("https://api.anthropic.com/api/oauth/usage")
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => return Err(format!("Network error: {e}")),
    };

    let status = resp.status();

    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Ok(SubscriptionQuota::error(
            "claude",
            CredentialStatus::Expired,
            format!("Authentication failed (HTTP {status}). Please re-login with Claude CLI."),
        ));
    }

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Ok(SubscriptionQuota::error(
            "claude",
            CredentialStatus::Valid,
            format!("API error (HTTP {status}): {body}"),
        ));
    }

    // 先 bytes() 再解析：读体失败（超时/连接中断）是瞬时 → Err；拿到完整响应体
    // 后解析失败才是确定性。reqwest 的 json() 把读体错误也包成 decode，无法区分。
    let raw = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => return Err(format!("Failed to read API response: {e}")),
    };
    let body: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => {
            return Ok(SubscriptionQuota::error(
                "claude",
                CredentialStatus::Valid,
                format!("Failed to parse API response: {e}"),
            ));
        }
    };

    Ok(parse_claude_quota(&body))
}

/// 兼容旧顶层窗口与新版模型专属周限额，保持查询、缓存和 UI 共用 QuotaTier。
fn parse_claude_quota(body: &serde_json::Value) -> SubscriptionQuota {
    // 解析已知的 tier 窗口
    let mut tiers = Vec::new();
    for &tier_name in KNOWN_TIERS {
        if let Some(window) = body.get(tier_name) {
            if let Ok(w) = serde_json::from_value::<ApiUsageWindow>(window.clone()) {
                if let Some(util) = w.utilization {
                    tiers.push(QuotaTier {
                        name: tier_name.to_string(),
                        utilization: util,
                        resets_at: w.resets_at,
                        used_value_usd: None,
                        max_value_usd: None,
                    });
                }
            }
        }
    }

    // 也解析未知窗口（API 可能返回新的窗口类型）
    if let Some(obj) = body.as_object() {
        for (key, value) in obj {
            if key == "extra_usage" || key == "limits" || KNOWN_TIERS.contains(&key.as_str()) {
                continue;
            }
            if let Ok(w) = serde_json::from_value::<ApiUsageWindow>(value.clone()) {
                if let Some(util) = w.utilization {
                    tiers.push(QuotaTier {
                        name: key.clone(),
                        utilization: util,
                        resets_at: w.resets_at,
                        used_value_usd: None,
                        max_value_usd: None,
                    });
                }
            }
        }
    }

    // 新版模型专属额度覆盖同名旧窗口。逐条解析，单个异常项目不影响其余额度。
    let mut scoped_tiers = HashSet::new();
    if let Some(limits) = body.get("limits").and_then(serde_json::Value::as_array) {
        for limit in limits {
            if limit.get("kind").and_then(serde_json::Value::as_str) != Some("weekly_scoped")
                || limit.get("group").and_then(serde_json::Value::as_str) != Some("weekly")
                // 不把特定使用场景的子限额合并进整个模型的周限额。
                || limit.pointer("/scope/surface").is_some_and(|v| !v.is_null())
            {
                continue;
            }
            let Some(model) = limit
                .pointer("/scope/model/display_name")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let tier_name = match model.trim().to_ascii_lowercase().as_str() {
                "fable" => TIER_SEVEN_DAY_FABLE,
                "opus" => TIER_SEVEN_DAY_OPUS,
                "sonnet" => TIER_SEVEN_DAY_SONNET,
                _ => continue,
            };
            let Ok(window) = serde_json::from_value::<ApiScopedUsageWindow>(limit.clone()) else {
                continue;
            };
            if !window.percent.is_finite()
                || window.percent < 0.0
                || !scoped_tiers.insert(tier_name)
            {
                continue;
            }
            // 与 Claude Code 一致：不按 is_active 过滤。0% / resets_at:null
            // 也可能是有效的模型额度；不存在的额度由接口省略。
            let tier = QuotaTier {
                name: tier_name.to_string(),
                utilization: window.percent,
                resets_at: window.resets_at,
                used_value_usd: None,
                max_value_usd: None,
            };
            if let Some(existing) = tiers.iter_mut().find(|t| t.name == tier_name) {
                *existing = tier;
            } else {
                tiers.push(tier);
            }
        }
    }
    tiers.sort_by_key(|tier| {
        KNOWN_TIERS
            .iter()
            .position(|&name| name == tier.name)
            .unwrap_or(KNOWN_TIERS.len())
    });

    // 解析超额使用
    let extra_usage = body.get("extra_usage").and_then(|v| {
        serde_json::from_value::<ApiExtraUsage>(v.clone())
            .ok()
            .map(|e| ExtraUsage {
                is_enabled: e.is_enabled.unwrap_or(false),
                monthly_limit: e.monthly_limit,
                used_credits: e.used_credits,
                utilization: e.utilization,
                currency: e.currency,
            })
    });

    SubscriptionQuota {
        tool: "claude".to_string(),
        credential_status: CredentialStatus::Valid,
        credential_message: None,
        success: true,
        tiers,
        extra_usage,
        reset_credits: None,
        credits_balance: None,
        error: None,
        queried_at: Some(now_millis()),
    }
}

/// `security` 找不到条目时的退出码（errSecItemNotFound）。
#[cfg(target_os = "macos")]
const SECURITY_ITEM_NOT_FOUND: i32 = 44;

// ── 入口函数 ──────────────────────────────────────────────

/// 按读到的 Claude 凭据查额度：访问令牌过期也先试一次（可能实际上还有效）。
async fn claude_quota_from(
    (token, status, message): (Option<String>, CredentialStatus, Option<String>),
) -> Result<SubscriptionQuota, String> {
    match status {
        CredentialStatus::NotFound => Ok(SubscriptionQuota::not_found("claude")),
        CredentialStatus::ParseError => Ok(SubscriptionQuota::error(
            "claude",
            CredentialStatus::ParseError,
            message.unwrap_or_else(|| "Failed to parse credentials".to_string()),
        )),
        CredentialStatus::Expired | CredentialStatus::RefreshPending => {
            if let Some(token) = token {
                let result = query_claude_quota(&token).await?;
                if result.success {
                    return Ok(result);
                }
            }
            Ok(SubscriptionQuota::error(
                "claude",
                status,
                message.unwrap_or_else(|| "OAuth token has expired".to_string()),
            ))
        }
        CredentialStatus::Valid => {
            let token = token.expect("token must be Some when status is Valid");
            query_claude_quota(&token).await
        }
    }
}






/// 查询指定 CLI 工具的官方订阅额度
///
/// 瞬时传输失败以 `Err` 传播（前端 reject → retry + 保留上次成功值）。Expired
/// 分支的"过期也试一把"重试同样用 `?` 传播瞬时错误——不能折叠成"已过期"，
/// 否则一次网络抖动会被误报成确定性的凭据过期。
pub async fn get_subscription_quota(tool: &str) -> Result<SubscriptionQuota, String> {
    match tool {
        "claude" | "anthropic" => claude_quota_from(read_claude_credentials()).await,
        _ => Ok(SubscriptionQuota::not_found(tool)),
    }
}

// ── 辅助函数 ──────────────────────────────────────────────

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

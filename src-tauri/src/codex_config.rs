use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::{
    atomic_write, delete_file, get_home_dir, path_is_within, read_json_file,
    write_json_file_private, write_text_file_private,
};
use crate::error::AppError;
use crate::model_capabilities::{image_input_capability_from_modalities, ImageInputCapability};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
#[cfg(not(test))]
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::process::{Command, Stdio};
use toml_edit::DocumentMut;

pub const CC_SWITCH_CODEX_MODEL_PROVIDER_ID: &str = "custom";
/// Temporary model-provider id used while the built-in `codex-official`
/// provider is routed through CC Switch.  A dedicated id is an ownership
/// marker: unlike a generic localhost `base_url`, it can be detected and
/// cleaned up without mistaking a user's own local provider for takeover.
pub const CC_SWITCH_CODEX_OFFICIAL_PROXY_PROVIDER_ID: &str = "cc-switch-official";
pub const CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME: &str = "cc-switch-model-catalog.json";

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Top-level `config.toml` key that controls Codex's built-in web-search tool.
pub(crate) const CODEX_WEB_SEARCH_FIELD: &str = "web_search";
/// Value that disables the web-search tool. Some native `/responses` gateways
/// reject a `web_search` tool with `responses_feature_not_supported` ("tool type
/// 'web_search' is not supported by this gateway phase"), so for those we write
/// this per the vendors' official Codex docs. Also doubles as cc-switch's
/// ownership sentinel: we only ever remove a `web_search` key whose value equals
/// this string, never a user's own setting.
pub(crate) const CODEX_WEB_SEARCH_DISABLED: &str = "disabled";

/// Native `/responses` gateways whose first-party models do NOT support the Codex
/// `web_search` hosted tool. A BLACKLIST (default-on): everything not listed keeps
/// Codex's default, so relays/aggregators fronting real GPT — and any unknown
/// provider — are never touched. This avoids a whitelist's dangerous failure mode
/// (a fragile "is this GPT?" heuristic wrongly keeping web_search ON → hard 400);
/// the blacklist's failure mode is the safe, recoverable one (a not-yet-listed
/// broken gateway errors once → add it here).
///
/// Matched two ways so an aggregator (e.g. SiliconFlow) fronting these vendors'
/// models is also caught:
/// - `base_url` host substring, and
/// - the model id's brand prefix (after stripping any `vendor/` path segment).
///
/// Verified 2026-06-28 doc audit — reject: MiMo (hard 400), LongCat (official
/// config ships `web_search = "disabled"`), MiniMax (tool-type enum `['function']`
/// only), and Qwen3-Coder models (百炼 marks built-in tools unsupported for
/// the coder series). Deliberately NOT listed by host: 火山方舟豆包, general
/// 阿里百炼 Qwen models that support built-in web_search, and GPT-native relays.
pub(crate) fn codex_url_host(url_or_host: &str) -> String {
    let trimmed = url_or_host.trim();
    let rest = trimmed
        .split_once("://")
        .map_or(trimmed, |(_scheme, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(ipv6) = host_port.strip_prefix('[') {
        ipv6.split(']').next().unwrap_or(ipv6)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Whether the URL's host IS one of `hosts` or a subdomain of it, matched on
/// DNS label boundaries. Vendor host lists must go through this rather than a
/// substring `contains`: a 4-char entry like `z.ai` would otherwise also match
/// `api.xyz.ai` / `viz.ai` and silently push an unrelated provider onto a
/// vendor-specific code path.
pub(crate) fn codex_url_host_matches_any(url_or_host: &str, hosts: &[&str]) -> bool {
    let host = codex_url_host(url_or_host);
    if host.is_empty() {
        return false;
    }
    hosts.iter().any(|candidate| {
        let candidate = candidate.trim_start_matches('.').to_ascii_lowercase();
        host == candidate || host.ends_with(&format!(".{candidate}"))
    })
}

/// Top-level `model` id from a Codex `config.toml`.
const CODEX_MANAGED_OAUTH_LIVE_AUTH_MARKER_FILENAME: &str = "codex_managed_oauth_live_auth.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodexManagedOAuthLiveAuthMarker {
    version: u32,
    /// cc-switch 本地托管账号 ID，用于区分同一 ChatGPT workspace 下的登录。
    account_id: String,
    /// 原生 auth.json 的 `tokens.account_id`，即 ChatGPT workspace ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chatgpt_account_id: Option<String>,
    /// id_token 中跨刷新稳定的用户身份，防止同 workspace 的原生登录串号。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    user_identity: Option<String>,
}

pub(crate) struct CodexManagedLiveRefresh {
    pub(crate) refresh_token: String,
    pub(crate) id_token: Option<String>,
    pub(crate) last_refresh_ms: Option<i64>,
    pub(crate) chatgpt_account_id: String,
}

const CODEX_RESERVED_MODEL_PROVIDER_IDS: &[&str] = &[
    "amazon-bedrock",
    "amazon-bedrock-runtime",
    "openai",
    "ollama",
    "lmstudio",
];

/// 获取 Codex 配置目录路径
pub fn get_codex_config_dir() -> PathBuf {
    if let Some(custom) = crate::settings::get_codex_override_dir() {
        return custom;
    }

    get_home_dir().join(".codex")
}

/// 获取 Codex auth.json 路径
pub fn get_codex_auth_path() -> PathBuf {
    get_codex_config_dir().join("auth.json")
}

pub(crate) fn get_codex_managed_oauth_live_auth_marker_path() -> PathBuf {
    crate::config::get_app_config_dir().join(CODEX_MANAGED_OAUTH_LIVE_AUTH_MARKER_FILENAME)
}

#[cfg(test)]
pub(crate) fn codex_managed_oauth_live_auth_marker_exists() -> bool {
    get_codex_managed_oauth_live_auth_marker_path().exists()
}

/// 从 live/备份的 Codex `auth` 中提取上游 ChatGPT workspace ID。
///
/// 仅接受 ChatGPT 登录形状（`auth_mode == "chatgpt"`、`OPENAI_API_KEY` 可清空）。
/// 托管账号写入的完整 bundle 会额外带 `tokens.refresh_token` 与顶层 `last_refresh`，
/// 这里一并容忍。Codex CLI 自刷新会轮换 access_token，因此短期 token 指纹不能
/// 作为稳定的所有权谓词；cc-switch 的本地账号 ID 单独记录在 marker 中。
fn extract_codex_managed_oauth_account_id(auth: &Value) -> Option<String> {
    let auth_obj = auth.as_object()?;

    if auth_obj.keys().any(|key| {
        !matches!(
            key.as_str(),
            "auth_mode" | "OPENAI_API_KEY" | "tokens" | "last_refresh"
        )
    }) {
        return None;
    }

    if auth.get("auth_mode").and_then(|value| value.as_str()) != Some("chatgpt") {
        return None;
    }

    let api_key_is_clearable = auth
        .get("OPENAI_API_KEY")
        .is_none_or(|value| value.is_null() || value.as_str() == Some("PROXY_MANAGED"));
    if !api_key_is_clearable {
        return None;
    }

    let tokens = auth.get("tokens").and_then(|value| value.as_object())?;

    if tokens.keys().any(|key| {
        !matches!(
            key.as_str(),
            "access_token" | "account_id" | "id_token" | "refresh_token"
        )
    }) {
        return None;
    }

    let account_id = tokens
        .get("account_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    tokens
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|token| !token.is_empty())?;

    Some(account_id.to_string())
}

/// 从原生 auth.json 的 id_token 提取跨刷新稳定的用户身份。
pub(crate) fn extract_codex_auth_user_identity(auth: &Value) -> Option<String> {
    let id_token = auth.pointer("/tokens/id_token")?.as_str()?;
    extract_codex_id_token_user_identity(id_token)
}

pub(crate) fn extract_codex_id_token_user_identity(id_token: &str) -> Option<String> {
    extract_codex_id_token_subject(id_token).map(|subject| format!("sub:{subject}"))
}

pub(crate) fn extract_codex_id_token_subject(id_token: &str) -> Option<String> {
    let mut segments = id_token.split('.');
    let header = segments.next()?;
    let payload = segments.next()?;
    segments.next()?;
    if segments.next().is_some() {
        return None;
    }

    let header: Value = URL_SAFE_NO_PAD
        .decode(header)
        .ok()
        .and_then(|decoded| serde_json::from_slice(&decoded).ok())?;
    header
        .get("alg")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    let claims: Value = URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|decoded| serde_json::from_slice(&decoded).ok())?;
    claims
        .get("sub")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) fn test_codex_id_token(subject: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
    let payload = URL_SAFE_NO_PAD.encode(json!({ "sub": subject }).to_string());
    format!("{header}.{payload}.")
}

/// Build the native-shaped ChatGPT auth bundle shared by cc-switch and Codex CLI.
pub fn codex_managed_oauth_auth_value(
    account_id: &str,
    access_token: &str,
    id_token: Option<&str>,
    refresh_token: &str,
    last_refresh: &str,
) -> Value {
    let mut tokens = serde_json::Map::new();
    if let Some(id_token) = id_token {
        tokens.insert("id_token".to_string(), Value::String(id_token.to_string()));
    }
    tokens.insert(
        "access_token".to_string(),
        Value::String(access_token.to_string()),
    );
    tokens.insert(
        "refresh_token".to_string(),
        Value::String(refresh_token.to_string()),
    );
    tokens.insert(
        "account_id".to_string(),
        Value::String(account_id.to_string()),
    );
    json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": Value::Object(tokens),
        "last_refresh": last_refresh,
    })
}

/// 托管账号登录标记的内容（和 [`record_codex_managed_oauth_live_auth`] 写的一样），给切换
/// 操作放进同一次提交用。`auth` 不是托管账号的登录形状时返回 `None`。
pub fn record_codex_managed_oauth_live_auth(
    auth: &Value,
    managed_account_id: &str,
) -> Result<(), AppError> {
    let managed_account_id = managed_account_id.trim();
    let Some(chatgpt_account_id) = extract_codex_managed_oauth_account_id(auth) else {
        return Ok(());
    };
    if managed_account_id.is_empty() {
        return Ok(());
    }
    let user_identity = extract_codex_auth_user_identity(auth).ok_or_else(|| {
        AppError::Message(
            "Codex 托管 OAuth auth.json 的 id_token 缺少稳定用户身份，无法安全记录账号所有权"
                .to_string(),
        )
    })?;

    let marker = CodexManagedOAuthLiveAuthMarker {
        version: 3,
        account_id: managed_account_id.to_string(),
        chatgpt_account_id: Some(chatgpt_account_id),
        user_identity: Some(user_identity),
    };
    crate::config::write_json_file(&get_codex_managed_oauth_live_auth_marker_path(), &marker)
}

fn migrate_legacy_codex_managed_oauth_live_auth_marker(
    auth: &Value,
    managed_account_id: &str,
    managed_id_token: Option<&str>,
) -> Result<(), AppError> {
    let marker_path = get_codex_managed_oauth_live_auth_marker_path();
    if !marker_path.exists() {
        return Ok(());
    }
    let marker: CodexManagedOAuthLiveAuthMarker = read_json_file(&marker_path)?;
    if !matches!(marker.version, 1 | 2) || marker.account_id != managed_account_id {
        return Ok(());
    }

    let auth_account_id = extract_codex_managed_oauth_account_id(auth);
    let auth_user_identity = extract_codex_auth_user_identity(auth);
    let managed_user_identity = managed_id_token.and_then(extract_codex_id_token_user_identity);
    if auth_account_id.as_deref() != Some(managed_account_id)
        || auth_user_identity.as_deref() != managed_user_identity.as_deref()
        || managed_user_identity.is_none()
    {
        return Err(AppError::Message(format!(
            "旧版 Codex OAuth 账号 {managed_account_id} 无法通过稳定用户身份确认磁盘凭据所有权；为避免覆盖或串用 auth.json，本次操作已取消，请在认证中心重新登录该账号"
        )));
    }

    record_codex_managed_oauth_live_auth(auth, managed_account_id)
}

/// Before removing a manager record, make any legacy live-auth ownership
/// provable with the manager's persisted user identity. Failure is surfaced so
/// callers keep the manager record and marker instead of orphaning auth.json.
pub(crate) fn prepare_codex_live_auth_for_managed_account_removal(
    managed_account_id: &str,
    managed_id_token: Option<&str>,
) -> Result<(), AppError> {
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Ok(());
    }
    let auth: Value = read_json_file(&auth_path)?;
    migrate_legacy_codex_managed_oauth_live_auth_marker(&auth, managed_account_id, managed_id_token)
}

pub fn codex_auth_matches_recorded_managed_oauth(
    auth: &Value,
    account_id: &str,
) -> Result<bool, AppError> {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return Ok(false);
    }

    let Some(auth_account_id) = extract_codex_managed_oauth_account_id(auth) else {
        return Ok(false);
    };
    let auth_user_identity = extract_codex_auth_user_identity(auth);
    let marker_path = get_codex_managed_oauth_live_auth_marker_path();
    // No marker is the normal state for a native ChatGPT login (no managed
    // account ever wrote auth.json, or it was cleaned up when switching away);
    // only a marker that exists but cannot be read is worth a warning.
    if !marker_path.exists() {
        return Ok(false);
    }
    let marker: CodexManagedOAuthLiveAuthMarker = match read_json_file(&marker_path) {
        Ok(marker) => marker,
        Err(err) => {
            log::warn!(
                "Failed to read Codex managed OAuth auth marker at {}: {err}",
                marker_path.display()
            );
            return Ok(false);
        }
    };

    // v1/v2 markers do not carry a stable user identity. Since multiple users
    // can share one workspace, those markers cannot safely authorize adopting
    // or deleting credentials. The next explicit activation replaces them
    // with a v3 marker.
    Ok(marker.account_id == account_id
        && match marker.version {
            3 => {
                marker.chatgpt_account_id.as_deref() == Some(auth_account_id.as_str())
                    && marker
                        .user_identity
                        .as_deref()
                        .is_some_and(|identity| auth_user_identity.as_deref() == Some(identity))
            }
            _ => false,
        })
}

/// Verify that a proxied Codex request still uses the exact live access token
/// owned by the selected local account. Workspace IDs alone are not sufficient:
/// different Team users can share one value.
pub(crate) fn codex_live_auth_matches_managed_request(
    account_id: &str,
    request_access_token: &str,
) -> Result<bool, AppError> {
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Ok(false);
    }
    let auth: Value = read_json_file(&auth_path)?;
    if !codex_auth_matches_recorded_managed_oauth(&auth, account_id)? {
        return Ok(false);
    }
    let live_access_token = auth
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty());
    Ok(live_access_token == Some(request_access_token.trim()))
}

pub(crate) fn clear_codex_managed_oauth_live_auth_marker_for_account(
    account_id: &str,
) -> Result<(), AppError> {
    let marker_path = get_codex_managed_oauth_live_auth_marker_path();
    if !marker_path.exists() {
        return Ok(());
    }
    let marker: CodexManagedOAuthLiveAuthMarker = match read_json_file(&marker_path) {
        Ok(marker) => marker,
        Err(error) => {
            log::warn!(
                "Failed to read Codex managed OAuth auth marker at {} while cleaning account {}: {error}",
                marker_path.display(),
                account_id
            );
            // A malformed marker cannot establish ownership for any account
            // and is unusable for rollback/synchronization; remove the stale
            // bookkeeping file while leaving non-matching live auth untouched.
            return delete_file(&marker_path);
        }
    };
    if marker.account_id == account_id.trim() {
        delete_file(&marker_path)?;
    }
    Ok(())
}

/// 切走托管 provider 或从认证中心删除账号时，清理其残留在
/// `~/.codex/auth.json` 的 ChatGPT 登录。
///
/// 删除谓词同时校验 cc-switch marker 中的本地账号 ID 与原生 auth.json 中的
/// workspace ID，不依赖会被 Codex CLI 自刷新破坏的 access-token 指纹。切换路径必须
/// 先把盘上轮换后的 refresh token 采纳回 manager，再调用本函数。
pub fn clear_codex_live_auth_for_managed_account(account_id: &str) -> Result<(), AppError> {
    clear_codex_live_auth_for_managed_account_if_unchanged(account_id, None)
}

/// Verify that the outgoing account's live refresh generation has not changed
/// since it was adopted into the OAuth manager.
pub fn ensure_codex_live_auth_unchanged_for_managed_account(
    account_id: &str,
    expected_refresh_token: &str,
) -> Result<(), AppError> {
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Err(AppError::Message(format!(
            "Codex CLI 账号 {account_id} 的 live auth 已在切换期间被移除，请重试"
        )));
    }
    let auth: Value = read_json_file(&auth_path)?;
    let current_refresh_token = auth
        .pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .map(str::trim);
    if !codex_live_auth_is_managed_chatgpt_login(&auth, account_id)
        || current_refresh_token != Some(expected_refresh_token.trim())
    {
        return Err(AppError::Message(format!(
            "Codex CLI 账号 {account_id} 的 live 凭据在切换期间已刷新；为避免覆盖新 refresh token，本次操作已取消，请重试"
        )));
    }
    Ok(())
}

/// Content-based cleanup with an optional compare-before-delete guard.
pub fn clear_codex_live_auth_for_managed_account_if_unchanged(
    account_id: &str,
    expected_refresh_token: Option<&str>,
) -> Result<(), AppError> {
    let auth_path = get_codex_auth_path();
    let mut removed_matching_auth = false;
    if auth_path.exists() {
        let auth: Value = read_json_file(&auth_path)?;
        if codex_live_auth_is_managed_chatgpt_login(&auth, account_id) {
            if let Some(expected_refresh_token) = expected_refresh_token {
                let current_refresh_token = auth
                    .pointer("/tokens/refresh_token")
                    .and_then(Value::as_str)
                    .map(str::trim);
                if current_refresh_token != Some(expected_refresh_token.trim()) {
                    return Err(AppError::Message(format!(
                        "Codex CLI 账号 {account_id} 的 live 凭据在切换期间已刷新；为避免删除新 refresh token，本次操作已取消，请重试"
                    )));
                }
            }
            delete_file(&auth_path)?;
            removed_matching_auth = true;
        }
    }

    if removed_matching_auth {
        // Once the matching live file is gone, any marker is stale regardless
        // of version or parseability.
        delete_file(&get_codex_managed_oauth_live_auth_marker_path())?;
    } else {
        clear_codex_managed_oauth_live_auth_marker_for_account(account_id)?;
    }
    Ok(())
}

/// 判断给定的 Codex `auth` 是否属于指定的 cc-switch 本地托管账号。
///
/// 原生 `tokens.account_id` 是 workspace ID，可能被多个本地账号共享；因此必须同时
/// 命中 cc-switch marker 中的本地账号 ID，不能只按 auth.json 内容判断。
///
/// 用于 Live 备份剥离：避免把托管账号的可刷新 token 持久化进备份配置。
pub fn codex_live_auth_is_managed_chatgpt_login(auth: &Value, account_id: &str) -> bool {
    codex_auth_matches_recorded_managed_oauth(auth, account_id).unwrap_or(false)
}

/// 读回 Codex CLI 当前 `~/.codex/auth.json` 中属于 `account_id` 的 refresh_token /
/// id_token（仅当磁盘上的登录账号与之一致时）。
///
/// 用于切换回托管 provider 前，采纳 CLI 自行刷新时轮换出的最新 refresh_token，避免
/// 用陈腐 token 覆盖 CLI 的有效登录（“裸跑 codex” 反复切换场景）。
pub fn read_codex_live_auth_refresh_for_account(
    account_id: &str,
) -> Option<(String, Option<String>, Option<i64>)> {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return None;
    }
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return None;
    }
    let auth: Value = read_json_file(&auth_path).ok()?;
    // 仅在磁盘上确是「该 account_id 的 ChatGPT 登录」时才采纳其 refresh_token，
    // 避免从非 chatgpt/异常 auth 里误取 token。
    if !codex_live_auth_is_managed_chatgpt_login(&auth, account_id) {
        return None;
    }
    let tokens = auth.get("tokens")?.as_object()?;
    let refresh_token = tokens.get("refresh_token")?.as_str()?.trim().to_string();
    if refresh_token.is_empty() {
        return None;
    }
    let id_token = tokens
        .get("id_token")
        .and_then(|value| value.as_str())
        .map(|token| token.to_string());
    let last_refresh_ms = auth
        .get("last_refresh")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis());
    Some((refresh_token, id_token, last_refresh_ms))
}

/// Read a managed live credential after safely upgrading a legacy marker.
/// v1/v2 markers only identify a workspace, so the manager's persisted
/// id_token must prove the live user's identity before the marker can become
/// authoritative again.
pub(crate) fn read_codex_live_auth_refresh_for_managed_account(
    account_id: &str,
    managed_id_token: Option<&str>,
) -> Result<Option<CodexManagedLiveRefresh>, AppError> {
    let account_id = account_id.trim();
    if account_id.is_empty() {
        return Ok(None);
    }
    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Ok(None);
    }
    let auth: Value = read_json_file(&auth_path)?;
    migrate_legacy_codex_managed_oauth_live_auth_marker(&auth, account_id, managed_id_token)?;
    if !codex_auth_matches_recorded_managed_oauth(&auth, account_id)? {
        return Ok(None);
    }
    let Some((refresh_token, id_token, last_refresh_ms)) =
        read_codex_live_auth_refresh_for_account(account_id)
    else {
        return Ok(None);
    };
    let chatgpt_account_id = extract_codex_managed_oauth_account_id(&auth)
        .ok_or_else(|| AppError::Message("Codex live auth 缺少 workspace ID".to_string()))?;
    Ok(Some(CodexManagedLiveRefresh {
        refresh_token,
        id_token,
        last_refresh_ms,
        chatgpt_account_id,
    }))
}

/// Keep Codex CLI's live auth in the same refresh-token generation after the
/// manager refreshes a managed account.
///
/// The write is compare-and-swap-like: immediately before replacing auth.json,
/// it verifies that the file still contains the refresh token used for the
/// network request. Codex CLI does not share cc-switch's process lock, so this
/// is a best-effort guard that narrows (but cannot make atomic) the cross-process
/// check-to-replace window.
/// Ownership is local-account scoped through the marker, while auth.json keeps
/// the upstream workspace ID required by Codex.
pub fn sync_codex_managed_oauth_live_auth_after_refresh(
    account_id: &str,
    expected_refresh_token: &str,
    refreshed_auth: &Value,
) -> Result<bool, AppError> {
    let account_id = account_id.trim();
    let expected_refresh_token = expected_refresh_token.trim();
    if account_id.is_empty() || expected_refresh_token.is_empty() {
        return Ok(false);
    }

    let auth_path = get_codex_auth_path();
    if !auth_path.exists() {
        return Ok(false);
    }
    let current_auth: Value = read_json_file(&auth_path)?;
    if !codex_live_auth_is_managed_chatgpt_login(&current_auth, account_id) {
        return Ok(false);
    }
    let current_refresh_token = current_auth
        .pointer("/tokens/refresh_token")
        .and_then(Value::as_str)
        .map(str::trim);
    if current_refresh_token != Some(expected_refresh_token) {
        return Ok(false);
    }

    let marker_path = get_codex_managed_oauth_live_auth_marker_path();
    let was_recorded_managed = marker_path.exists()
        && codex_auth_matches_recorded_managed_oauth(&current_auth, account_id)?;

    write_json_file_private(&auth_path, refreshed_auth)?;
    if was_recorded_managed {
        record_codex_managed_oauth_live_auth(refreshed_auth, account_id)?;
    }
    Ok(true)
}

/// 获取 Codex config.toml 路径
pub fn get_codex_config_path() -> PathBuf {
    get_codex_config_dir().join("config.toml")
}

pub fn write_codex_live_atomic(
    auth: &Value,
    config_text_opt: Option<&str>,
) -> Result<(), AppError> {
    let auth_path = get_codex_auth_path();
    let config_path = get_codex_config_path();

    if let Some(parent) = auth_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    // 读取旧内容用于回滚
    let old_auth = if auth_path.exists() {
        Some(fs::read(&auth_path).map_err(|e| AppError::io(&auth_path, e))?)
    } else {
        None
    };
    let _old_config = if config_path.exists() {
        Some(fs::read(&config_path).map_err(|e| AppError::io(&config_path, e))?)
    } else {
        None
    };

    // 准备写入内容
    let cfg_text = match config_text_opt {
        Some(s) => s.to_string(),
        None => String::new(),
    };
    if !cfg_text.trim().is_empty() {
        toml::from_str::<toml::Table>(&cfg_text).map_err(|e| AppError::toml(&config_path, e))?;
    }

    // 第一步：写 auth.json
    write_json_file_private(&auth_path, auth)?;

    // 第二步：写 config.toml（失败则回滚 auth.json）
    if let Err(e) = write_text_file_private(&config_path, &cfg_text) {
        // 回滚 auth.json
        if let Some(bytes) = old_auth {
            let _ = atomic_write(&auth_path, &bytes);
        } else {
            let _ = delete_file(&auth_path);
        }
        return Err(e);
    }

    Ok(())
}

/// 读取 `~/.codex/config.toml`，若不存在返回空字符串
pub fn read_codex_config_text() -> Result<String, AppError> {
    let path = get_codex_config_path();
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| AppError::io(&path, e))
    } else {
        Ok(String::new())
    }
}

/// `config.toml` 是不是用 `[features] multi_agent_v2` 强制了新版子 agent 工具：写成 `true`，
/// 或写成带 `enabled = true` 的表（codex-rs `features` 的 `FeatureToml`）。Codex 先看它再看
/// 模型目录，打开了它，目录里写的 [`CODEX_CLASSIC_SUBAGENTS`] 就不生效。解析不了时按没有。
pub(crate) fn codex_config_forces_multi_agent_v2(text: &str) -> bool {
    codex_config_feature_enabled(text, "multi_agent_v2")
}

/// `config.toml` 的 `[features] <key>` 是否开启：写成 `true`，或写成带 `enabled = true`
/// 的表（codex-rs `features` 的 `FeatureToml`）。解析不了时按没开。
pub(crate) fn codex_config_feature_enabled(text: &str, key: &str) -> bool {
    let Ok(doc) = text.parse::<DocumentMut>() else {
        return false;
    };
    let Some(item) = doc.get("features").and_then(|features| features.get(key)) else {
        return false;
    };
    item.as_bool()
        .or_else(|| item.get("enabled").and_then(toml_edit::Item::as_bool))
        .unwrap_or(false)
}

/// 对非空的 TOML 文本进行语法校验
pub fn validate_config_toml(text: &str) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Ok(());
    }
    toml::from_str::<toml::Table>(text)
        .map(|_| ())
        .map_err(|e| AppError::toml(Path::new("config.toml"), e))
}

/// 读取并校验 `~/.codex/config.toml`，返回文本（可能为空）
pub fn read_and_validate_codex_config_text() -> Result<String, AppError> {
    let s = read_codex_config_text()?;
    validate_config_toml(&s)?;
    Ok(s)
}

fn active_codex_model_provider_id(doc: &DocumentMut) -> Option<String> {
    doc.get("model_provider")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

pub(crate) fn is_custom_codex_model_provider_id(id: &str) -> bool {
    // Exact match, mirroring upstream: both the built-in provider lookup and
    // validate_reserved_model_provider_ids are case-sensitive, so `OpenAI`
    // etc. are legitimate custom ids whose tables must receive the token.
    // Keep in sync with the frontend list in src/utils/providerConfigUtils.ts.
    let id = id.trim();
    !id.is_empty() && !CODEX_RESERVED_MODEL_PROVIDER_IDS.contains(&id)
}

pub fn extract_codex_auth_api_key(auth: &Value) -> Option<String> {
    auth.get("OPENAI_API_KEY")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

pub fn extract_codex_api_key(auth: Option<&Value>, config_text: Option<&str>) -> Option<String> {
    auth.and_then(extract_codex_auth_api_key)
        .or_else(|| config_text.and_then(extract_codex_experimental_bearer_token))
}

/// Extract the upstream base URL from a Codex `config.toml` string.
///
/// Prefers the active `[model_providers.<model_provider>].base_url`, falling
/// back to a top-level `base_url`. Deliberately never reads a non-active
/// `[model_providers.*]` section — the frontend `extractCodexBaseUrl`
/// (`getRecoverableBaseUrlAssignments`) excludes those too, and a leftover
/// section unrelated to the active provider must not leak into `{{baseUrl}}`.
pub fn extract_codex_base_url(config_text: &str) -> Option<String> {
    let doc = config_text.parse::<toml::Value>().ok()?;

    if let Some(active_provider) = doc.get("model_provider").and_then(|v| v.as_str()) {
        if let Some(base_url) = doc
            .get("model_providers")
            .and_then(|providers| providers.get(active_provider))
            .and_then(|provider| provider.get("base_url"))
            .and_then(|v| v.as_str())
        {
            return Some(base_url.to_string());
        }
    }

    doc.get("base_url")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

const MAX_CODEX_CATALOG_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) fn read_limited_string(path: &Path, max_bytes: u64) -> Result<String, AppError> {
    let metadata = fs::metadata(path).map_err(|error| AppError::io(path, error))?;
    if metadata.len() > max_bytes {
        return Err(AppError::Config(format!(
            "文件 {} 超过大小上限 {} 字节",
            path.display(),
            max_bytes
        )));
    }
    fs::read_to_string(path).map_err(|error| AppError::io(path, error))
}

/// Read the cc-switch Codex model catalog file with a size cap.
pub(crate) fn read_codex_model_catalog_text(path: &Path) -> Result<String, AppError> {
    read_limited_string(path, MAX_CODEX_CATALOG_BYTES)
}

/// Given `config.toml` text, resolve the on-disk path of the cc-switch–owned
/// catalog file (returns `None` if `model_catalog_json` is absent or points at
/// a file we don't own). Relative paths are resolved under `base_dir`;
/// absolute paths must still be inside `base_dir`.
pub(crate) fn resolve_cc_switch_catalog_path(
    config_text: &str,
    base_dir: &Path,
) -> Option<PathBuf> {
    if config_text.trim().is_empty() {
        return None;
    }
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let catalog_path_str = doc
        .get("model_catalog_json")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;

    let referenced_path = Path::new(catalog_path_str);
    let is_cc_switch_owned = referenced_path.file_name().and_then(|name| name.to_str())
        == Some(CC_SWITCH_CODEX_MODEL_CATALOG_FILENAME);
    if !is_cc_switch_owned {
        return None;
    }

    // 注意（有意的行为变更）：Windows 上 `/…` 形式的旧 WSL 风格 Linux 路径也会
    // 被视为绝对路径，从而在下方的包含性校验中失败——此前这类路径会因无法匹配
    // 生成文件名而回退为按文件名解析、碰巧能工作。可接受：下一次切换供应商时
    // 写入侧会重新落一个裸文件名，配置自愈（见
    // `set_catalog_json_none_removes_cc_switch_owned_by_filename` 的场景注释）。
    let is_unix_absolute = catalog_path_str.starts_with('/');
    let resolved = if referenced_path.is_absolute() || is_unix_absolute {
        referenced_path.to_path_buf()
    } else {
        base_dir.join(referenced_path)
    };

    if !path_is_within(base_dir, &resolved) {
        log::warn!(
            "Codex model_catalog_json 指向配置目录外: {}（允许目录: {}）",
            resolved.display(),
            base_dir.display()
        );
        return None;
    }

    // 词法包含不等于运行时包含：配置目录内的符号链接（如 ~/.codex/link ->
    // /etc）能让 `link/cc-switch-model-catalog.json` 通过上面的检查，读取却
    // 落到目录外。文件存在时把真实路径 canonicalize 出来再校验一次，并把
    // canonical 路径返回给调用方——后续读取不再经过 symlink 组件。
    if resolved.exists() {
        let canonical = match fs::canonicalize(&resolved) {
            Ok(path) => path,
            Err(error) => {
                log::warn!(
                    "Codex model_catalog_json canonicalize 失败: {}: {error}",
                    resolved.display()
                );
                return None;
            }
        };
        // base 同样 canonicalize，保证两侧前缀一致（Windows \\?\、
        // macOS /tmp -> /private/tmp）；base 失败时退回词法 base——
        // 词法 base 与 canonical 路径比较只会误拒（退化为不读），不会误放。
        let canonical_base = fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.to_path_buf());
        if !path_is_within(&canonical_base, &canonical) {
            log::warn!(
                "Codex model_catalog_json 经符号链接解析到配置目录外: {} -> {}（允许目录: {}）",
                resolved.display(),
                canonical.display(),
                canonical_base.display()
            );
            return None;
        }
        return Some(canonical);
    }

    Some(resolved)
}

/// Pure reverse-parsing core: convert Codex catalog JSON text back into the
/// frontend's simplified model-mapping shape. Returns `None` when the catalog
/// is unparseable, has no `models` array, or yields zero valid entries.
pub fn extract_codex_experimental_bearer_token(config_text: &str) -> Option<String> {
    if !config_text.contains("experimental_bearer_token") {
        return None;
    }
    let doc = config_text.parse::<DocumentMut>().ok()?;
    let provider_id = active_codex_model_provider_id(&doc);

    let top_level_token = || {
        doc.get("experimental_bearer_token")
            .and_then(|item| item.as_str())
    };
    let token = match provider_id.as_deref() {
        // `as_table_like` (not `as_table`): user configs may use inline tables
        // (`model_providers = { foo = {...} }`), which `as_table` rejects.
        Some(id) if is_custom_codex_model_provider_id(id) => doc
            .get("model_providers")
            .and_then(|item| item.as_table_like())
            .and_then(|table| table.get(id))
            .and_then(|item| item.as_table_like())
            .and_then(|table| table.get("experimental_bearer_token"))
            .and_then(|item| item.as_str())
            .or_else(top_level_token),
        Some(_) => top_level_token(),
        None => top_level_token(),
    };

    token
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

/// Read the current Codex live settings as a `{ auth, config }` object.
///
/// Missing `auth.json` collapses to `{}` so a config-only third-party install
/// is still importable; both files missing is treated as "no live install".
/// A `config.toml` that exists but is empty is a valid state — e.g. the
/// official seed after stale-auth cleanup — and must stay readable.
pub fn read_codex_live_settings() -> Result<Value, AppError> {
    let auth_path = get_codex_auth_path();
    let auth_present = auth_path.exists();
    let auth: Value = if auth_present {
        read_json_file(&auth_path)?
    } else {
        json!({})
    };
    let cfg_text = read_and_validate_codex_config_text()?;
    if !auth_present && !get_codex_config_path().exists() {
        return Err(AppError::localized(
            "codex.live.missing",
            "Codex 配置文件不存在",
            "Codex configuration is missing",
        ));
    }
    Ok(json!({ "auth": auth, "config": cfg_text }))
}

/// Whether a live Codex config is the official route projected by an older CC Switch
/// (`model_provider = "cc-switch-official"`).
fn table_matches_codex_unified_official_provider(table: &toml_edit::Table) -> bool {
    table.len() == 4
        && table.get("name").and_then(|item| item.as_str()) == Some("OpenAI")
        && table
            .get("requires_openai_auth")
            .and_then(|item| item.as_bool())
            == Some(true)
        && table
            .get("supports_websockets")
            .and_then(|item| item.as_bool())
            == Some(true)
        && table.get("wire_api").and_then(|item| item.as_str()) == Some("responses")
}

/// `inject_codex_unified_session_bucket` 的反向操作：从配置文本里剥掉注入的
/// 统一会话路由，保证切换回填不会把它带进数据库的存储配置（关闭开关后
/// 切换即可完全还原）。仅当形态与注入产物完全一致时才剥离；第三方模板和
/// 用户自定义的 `custom` 条目（带 base_url 等差异字段）原样保留。
pub fn strip_codex_unified_session_bucket(config_text: &str) -> Result<String, AppError> {
    if !config_text.contains("model_provider") {
        return Ok(config_text.to_string());
    }
    let mut doc = config_text
        .parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Invalid Codex config.toml: {e}")))?;

    if doc.get("model_provider").and_then(|item| item.as_str())
        != Some(CC_SWITCH_CODEX_MODEL_PROVIDER_ID)
    {
        return Ok(config_text.to_string());
    }
    let matches_injected = doc
        .get("model_providers")
        .and_then(|item| item.as_table())
        .and_then(|providers| providers.get(CC_SWITCH_CODEX_MODEL_PROVIDER_ID))
        .and_then(|item| item.as_table())
        .is_some_and(table_matches_codex_unified_official_provider);
    if !matches_injected {
        return Ok(config_text.to_string());
    }

    doc.as_table_mut().remove("model_provider");
    let providers_empty = doc["model_providers"]
        .as_table_mut()
        .map(|providers| {
            providers.remove(CC_SWITCH_CODEX_MODEL_PROVIDER_ID);
            providers.is_empty()
        })
        .unwrap_or(false);
    if providers_empty {
        doc.as_table_mut().remove("model_providers");
    }
    Ok(doc.to_string())
}



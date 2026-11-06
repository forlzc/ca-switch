use indexmap::IndexMap;
use tauri::{Emitter, Manager, State};

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::provider::EditorSave;
use crate::services::{
    EndpointLatency, ProviderService, ProviderSortUpdate, SpeedtestService, SwitchResult,
};
use crate::store::AppState;
use std::str::FromStr;

// 常量定义
const TEMPLATE_TYPE_TOKEN_PLAN: &str = "token_plan";
const TEMPLATE_TYPE_BALANCE: &str = "balance";
const TEMPLATE_TYPE_OFFICIAL_SUBSCRIPTION: &str = "official_subscription";

/// 获取所有供应商
#[tauri::command]
pub async fn get_providers(
    state: State<'_, AppState>,
    app: String,
) -> Result<IndexMap<String, Provider>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    // 启动会话同步也使用同一个数据库连接，等待它的锁不能占住 UI 线程。
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        ProviderService::list(&state, app_type).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("读取供应商列表任务失败: {e}"))?
}

#[tauri::command]
pub async fn get_current_provider(
    state: State<'_, AppState>,
    app: String,
) -> Result<String, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        ProviderService::current(&state, app_type).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("读取当前供应商任务失败: {e}"))?
}

#[tauri::command]
pub async fn add_provider(
    app_handle: tauri::AppHandle,
    app: String,
    provider: Provider,
    #[allow(non_snake_case)] editorSave: Option<EditorSave>,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle
            .try_state::<AppState>()
            .ok_or_else(|| "应用状态不可用".to_string())?;
        ProviderService::add_from_editor(state.inner(), app_type, provider, editorSave)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("供应商添加任务执行失败: {e}"))?
}

#[tauri::command]
pub async fn update_provider(
    app_handle: tauri::AppHandle,
    app: String,
    provider: Provider,
    #[allow(non_snake_case)] originalId: Option<String>,
    #[allow(non_snake_case)] editorSave: Option<EditorSave>,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle
            .try_state::<AppState>()
            .ok_or_else(|| "应用状态不可用".to_string())?;
        ProviderService::update_from_editor(
            state.inner(),
            app_type,
            originalId.as_deref(),
            provider,
            editorSave,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("供应商更新任务执行失败: {e}"))?
}

#[tauri::command]
pub async fn delete_provider(
    state: State<'_, AppState>,
    app: String,
    id: String,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        ProviderService::delete(&state, app_type, &id)
            .map(|_| true)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("供应商删除任务失败: {e}"))?
}

fn switch_provider_internal(
    state: &AppState,
    app_type: AppType,
    id: &str,
) -> Result<SwitchResult, AppError> {
    ProviderService::switch(state, app_type, id)
}

#[cfg_attr(not(feature = "test-hooks"), doc(hidden))]
pub fn switch_provider_test_hook(
    state: &AppState,
    app_type: AppType,
    id: &str,
) -> Result<SwitchResult, AppError> {
    switch_provider_internal(state, app_type, id)
}

#[tauri::command]
pub async fn switch_provider(
    app_handle: tauri::AppHandle,
    app: String,
    id: String,
) -> Result<SwitchResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    let handle = app_handle.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let state = handle
            .try_state::<AppState>()
            .ok_or_else(|| "应用状态不可用".to_string())?;
        switch_provider_internal(state.inner(), app_type, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("供应商切换任务执行失败: {e}"))??;
    Ok(result)
}









#[allow(non_snake_case)]
#[tauri::command]
pub async fn queryProviderUsage(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String, // 使用 camelCase 匹配前端
    app: String,
) -> Result<crate::provider::UsageResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    // inner 可能以两种形式失败：
    //   1) 返回 Ok(UsageResult { success: false, .. }) —— 确定性失败（401、脚本
    //      报错、未知供应商等）。写进 UsageCache 并刷新托盘，让
    //      format_script_summary 的 success 守卫生效、suffix 自然消失。
    //   2) 返回 Err(String) —— 瞬时传输失败（网络/超时）及 DB/Copilot fetch 等。
    //      不写失败快照、不 emit：保留上一份托盘快照，与前端 react-query reject
    //      保留上次 data 的语义一致；否则失败快照会经 useUsageCacheBridge 盲写
    //      回 query 缓存，抹掉 reject 本该保留的旧值。
    let inner = query_provider_usage_inner(&state, app_type.clone(), &providerId).await;
    if let Ok(snapshot) = &inner {
        let payload = serde_json::json!({
            "kind": "script",
            "appType": app_type.as_str(),
            "providerId": &providerId,
            "data": snapshot,
        });
        if let Err(e) = app_handle.emit("usage-cache-updated", payload) {
            log::error!("emit usage-cache-updated (script) 失败: {e}");
        }
        state
            .usage_cache
            .put_script(app_type, providerId, snapshot.clone());
        crate::tray::schedule_tray_refresh(&app_handle);
    }
    inner
}

/// Resolve `(base_url, api_key)` for native usage queries, delegating to the
/// per-app resolver on `Provider`. Missing provider → empty credentials.
fn resolve_native_credentials(app_type: &AppType, provider: Option<&Provider>) -> (String, String) {
    provider
        .map(|p| p.resolve_usage_credentials(app_type))
        .unwrap_or_default()
}

fn resolve_coding_plan_credentials(
    app_type: &AppType,
    provider: Option<&Provider>,
    usage_script: Option<&crate::provider::UsageScript>,
) -> (String, String) {
    let is_zenmux = usage_script
        .and_then(|s| s.coding_plan_provider.as_deref())
        .map(|provider| provider.eq_ignore_ascii_case("zenmux"))
        .unwrap_or(false);

    if !is_zenmux {
        return resolve_native_credentials(app_type, provider);
    }

    let script_base_url = usage_script
        .and_then(|s| s.base_url.as_deref())
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();
    let script_api_key = usage_script
        .and_then(|s| s.api_key.as_deref())
        .unwrap_or("")
        .to_string();

    if !script_base_url.is_empty() && !script_api_key.is_empty() {
        return (script_base_url, script_api_key);
    }

    let native = resolve_native_credentials(app_type, provider);
    if !native.0.is_empty() && !native.1.is_empty() {
        native
    } else {
        (script_base_url, script_api_key)
    }
}

async fn query_provider_usage_inner(
    state: &AppState,
    app_type: AppType,
    provider_id: &str,
) -> Result<crate::provider::UsageResult, String> {
    // 从数据库读取供应商信息，检查特殊模板类型
    let providers = state
        .db
        .get_all_providers(app_type.as_str())
        .map_err(|e| format!("Failed to get providers: {e}"))?;
    let provider = providers.get(provider_id);
    let usage_script = provider
        .and_then(|p| p.meta.as_ref())
        .and_then(|m| m.usage_script.as_ref());
    let template_type = usage_script
        .and_then(|s| s.template_type.as_deref())
        .unwrap_or("");

    // ── Coding Plan 专用路径 ──
    if template_type == TEMPLATE_TYPE_TOKEN_PLAN {
        let (base_url, api_key) =
            resolve_coding_plan_credentials(&app_type, provider, usage_script);

        // 火山方舟用账号 AK/SK 签名查询用量（存于 usage_script，与推理 api_key 分离）；
        // 其他供应商为 None，service 层沿用 api_key。
        let access_key_id = usage_script.and_then(|s| s.access_key_id.clone());
        let secret_access_key = usage_script.and_then(|s| s.secret_access_key.clone());
        // 智谱团队版：显式 provider 标识 + 组织/项目 ID（与个人版智谱 base_url 相同，
        // 靠 coding_plan_provider == "zhipu_team" 在 service 层路由）。
        let coding_plan_provider = usage_script.and_then(|s| s.coding_plan_provider.clone());
        let team_organization_id = usage_script.and_then(|s| s.team_organization_id.clone());
        let team_project_id = usage_script.and_then(|s| s.team_project_id.clone());

        let outcome = crate::services::coding_plan::query_coding_plan(
            &base_url,
            &api_key,
            access_key_id.as_deref(),
            secret_access_key.as_deref(),
            coding_plan_provider.as_deref(),
            team_organization_id.as_deref(),
            team_project_id.as_deref(),
        )
        .await
        .map_err(|e| format!("Failed to query coding plan: {e}"))?;

        // 智谱按量账户没有 Coding Plan：有套餐显示套餐，没有就显示账户余额（#8465），
        // 不再显示「查询失败」让人以为供应商坏了。卡片和托盘都按余额样式显示这条结果。
        let quota = match outcome {
            crate::services::coding_plan::CodingPlanOutcome::Quota(quota) => quota,
            crate::services::coding_plan::CodingPlanOutcome::ZhipuNoPlan(_) => {
                return crate::services::balance::query_zhipu_balance(&base_url, &api_key)
                    .await
                    .map_err(|e| format!("Failed to query balance: {e}"));
            }
        };

        // 将 SubscriptionQuota 转换为 UsageResult
        if !quota.success {
            return Ok(crate::provider::UsageResult {
                success: false,
                data: None,
                error: quota.error,
            });
        }

        // ZenMux 的 tier 携带 USD 额度信息，需要编码为 JSON extra
        let has_usd = quota
            .tiers
            .first()
            .map(|t| t.used_value_usd.is_some())
            .unwrap_or(false);
        let plan_label = quota
            .credential_message
            .as_deref()
            .and_then(|msg| msg.split(' ').next())
            .map(|tier| format!("ZenMux·{}", tier.to_uppercase()));
        let mut first_tier = true;

        let data: Vec<crate::provider::UsageData> = quota
            .tiers
            .iter()
            .map(|tier| {
                let total = 100.0;
                let used = tier.utilization;
                let remaining = total - used;
                let extra = if has_usd {
                    let mut extra_json = serde_json::json!({
                        "resetsAt": tier.resets_at,
                    });
                    if let Some(v) = tier.used_value_usd {
                        extra_json["usedValueUsd"] = serde_json::json!(v);
                    }
                    if let Some(v) = tier.max_value_usd {
                        extra_json["maxValueUsd"] = serde_json::json!(v);
                    }
                    if first_tier {
                        if let Some(ref label) = plan_label {
                            extra_json["planLabel"] = serde_json::json!(label);
                        }
                        first_tier = false;
                    }
                    Some(extra_json.to_string())
                } else {
                    tier.resets_at.clone()
                };
                crate::provider::UsageData {
                    plan_name: Some(tier.name.clone()),
                    remaining: Some(remaining),
                    total: Some(total),
                    used: Some(used),
                    unit: Some("%".to_string()),
                    is_valid: Some(true),
                    invalid_message: None,
                    extra,
                }
            })
            .collect();

        return Ok(crate::provider::UsageResult {
            success: true,
            data: if data.is_empty() { None } else { Some(data) },
            error: None,
        });
    }

    // ── 官方余额查询路径 ──
    if template_type == TEMPLATE_TYPE_BALANCE {
        // 按 app 区分的凭据存储格式提取 Base URL 与 API Key
        let (base_url, api_key) = resolve_native_credentials(&app_type, provider);

        return crate::services::balance::get_balance(&base_url, &api_key)
            .await
            .map_err(|e| format!("Failed to query balance: {e}"));
    }

    // ── 官方订阅额度查询路径 ──
    if template_type == TEMPLATE_TYPE_OFFICIAL_SUBSCRIPTION {
        if !usage_script.map(|s| s.enabled).unwrap_or(false) {
            return Ok(crate::provider::UsageResult {
                success: false,
                data: None,
                error: Some("Usage query is disabled".to_string()),
            });
        }

        let quota = crate::services::subscription::get_subscription_quota(app_type.as_str())
            .await
            .map_err(|e| format!("Failed to query subscription quota: {e}"))?;

        if !quota.success {
            return Ok(crate::provider::UsageResult {
                success: false,
                data: None,
                error: quota.error.or(quota.credential_message),
            });
        }

        let data: Vec<crate::provider::UsageData> = quota
            .tiers
            .iter()
            .map(|tier| crate::provider::UsageData {
                plan_name: Some(tier.name.clone()),
                remaining: Some(100.0 - tier.utilization),
                total: Some(100.0),
                used: Some(tier.utilization),
                unit: Some("%".to_string()),
                is_valid: Some(true),
                invalid_message: None,
                extra: tier.resets_at.clone(),
            })
            .collect();

        return Ok(crate::provider::UsageResult {
            success: true,
            data: if data.is_empty() { None } else { Some(data) },
            error: None,
        });
    }

    // ── 通用 JS 脚本路径 ──
    ProviderService::query_usage(state, app_type, provider_id)
        .await
        .map_err(|e| e.to_string())
}

#[allow(non_snake_case)]
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn testUsageScript(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String,
    app: String,
    #[allow(non_snake_case)] scriptCode: String,
    timeout: Option<u64>,
    #[allow(non_snake_case)] apiKey: Option<String>,
    #[allow(non_snake_case)] baseUrl: Option<String>,
    #[allow(non_snake_case)] accessToken: Option<String>,
    #[allow(non_snake_case)] userId: Option<String>,
    #[allow(non_snake_case)] templateType: Option<String>,
) -> Result<crate::provider::UsageResult, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::test_usage_script(
        state.inner(),
        app_type,
        &providerId,
        &scriptCode,
        timeout.unwrap_or(10),
        apiKey.as_deref(),
        baseUrl.as_deref(),
        accessToken.as_deref(),
        userId.as_deref(),
        templateType.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn test_api_endpoints(
    urls: Vec<String>,
    #[allow(non_snake_case)] timeoutSecs: Option<u64>,
) -> Result<Vec<EndpointLatency>, String> {
    SpeedtestService::test_endpoints(urls, timeoutSecs)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_custom_endpoints(
    state: State<'_, AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
) -> Result<Vec<crate::settings::CustomEndpoint>, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::get_custom_endpoints(state.inner(), app_type, &providerId)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_custom_endpoint(
    state: State<'_, AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    url: String,
) -> Result<(), String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::add_custom_endpoint(state.inner(), app_type, &providerId, url)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_custom_endpoint(
    state: State<'_, AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    url: String,
) -> Result<(), String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::remove_custom_endpoint(state.inner(), app_type, &providerId, url)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_endpoint_last_used(
    state: State<'_, AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    url: String,
) -> Result<(), String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    ProviderService::update_endpoint_last_used(state.inner(), app_type, &providerId, url)
        .map_err(|e| e.to_string())
}

/// 排序就是故障转移队列的优先级，托盘按它列队列：改完重建托盘。聚合名单也跟着重排，
/// 客户端看到的聚合模型顺序才和界面一致。
///
/// 整段串行：异步命令不再在主线程排队，两次拖动的逐行写入会交错成哪次都不是的顺序。
/// 聚合名单发布失败时报错（排序已经存下），再拖一次就会重试发布。
#[tauri::command]
pub async fn update_providers_sort_order(
    app_handle: tauri::AppHandle,
    state: State<'_, AppState>,
    app: String,
    updates: Vec<ProviderSortUpdate>,
) -> Result<bool, String> {
    static SORT_ORDER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    let _serial = SORT_ORDER_LOCK.lock().await;
    let changed = ProviderService::update_sort_order(state.inner(), app_type.clone(), updates)
        .map_err(|e| e.to_string())?;
    crate::tray::refresh_tray_menu(&app_handle);
    Ok(changed)
}

use crate::provider::UniversalProvider;
use std::collections::HashMap;
use tauri::AppHandle;

#[derive(Clone, serde::Serialize)]
pub struct UniversalProviderSyncedEvent {
    pub action: String,
    pub id: String,
}

fn emit_universal_provider_synced(app: &AppHandle, action: &str, id: &str) {
    let _ = app.emit(
        "universal-provider-synced",
        UniversalProviderSyncedEvent {
            action: action.to_string(),
            id: id.to_string(),
        },
    );
}

#[tauri::command]
pub fn get_universal_providers(
    state: State<'_, AppState>,
) -> Result<HashMap<String, UniversalProvider>, String> {
    ProviderService::list_universal(state.inner()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_universal_provider(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<UniversalProvider>, String> {
    ProviderService::get_universal(state.inner(), &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn upsert_universal_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    provider: UniversalProvider,
) -> Result<bool, String> {
    let id = provider.id.clone();
    let result =
        ProviderService::upsert_universal(state.inner(), provider).map_err(|e| e.to_string())?;

    emit_universal_provider_synced(&app, "upsert", &id);

    Ok(result)
}

#[tauri::command]
pub fn delete_universal_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    let result =
        ProviderService::delete_universal(state.inner(), &id).map_err(|e| e.to_string())?;

    emit_universal_provider_synced(&app, "delete", &id);

    Ok(result)
}

#[tauri::command]
pub fn sync_universal_provider(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    let result =
        ProviderService::sync_universal_to_apps(state.inner(), &id).map_err(|e| e.to_string())?;

    emit_universal_provider_synced(&app, "sync", &id);

    Ok(result)
}




// ============================================================================
// OpenClaw 专属命令 → 已迁移至 commands/openclaw.rs
// ============================================================================


#[cfg(test)]
mod native_query_credentials_tests {
    use super::{resolve_coding_plan_credentials, resolve_native_credentials};
    use crate::app_config::AppType;
    use crate::provider::{Provider, UsageScript};
    use serde_json::json;

    fn usage_script(
        coding_plan_provider: Option<&str>,
        base_url: Option<&str>,
        api_key: Option<&str>,
    ) -> UsageScript {
        UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: String::new(),
            timeout: Some(10),
            api_key: api_key.map(str::to_string),
            base_url: base_url.map(str::to_string),
            access_token: None,
            user_id: None,
            template_type: Some("token_plan".to_string()),
            auto_query_interval: None,
            coding_plan_provider: coding_plan_provider.map(str::to_string),
            access_key_id: None,
            secret_access_key: None,
            team_organization_id: None,
            team_project_id: None,
        }
    }

    #[test]
    fn missing_provider_yields_empty() {
        let (base_url, api_key) = resolve_native_credentials(&AppType::Claude, None);
        assert!(base_url.is_empty());
        assert!(api_key.is_empty());
    }

    #[test]
    fn zenmux_coding_plan_uses_script_credentials_first() {
        let provider = Provider::with_id(
            "test".to_string(),
            "Test".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://provider.zenmux.example/v1",
                    "ANTHROPIC_AUTH_TOKEN": "sk-provider"
                }
            }),
            None,
        );
        let script = usage_script(
            Some("zenmux"),
            Some("https://script.zenmux.example/api/usage/"),
            Some("sk-script"),
        );

        let (base_url, api_key) =
            resolve_coding_plan_credentials(&AppType::Claude, Some(&provider), Some(&script));

        assert_eq!(base_url, "https://script.zenmux.example/api/usage");
        assert_eq!(api_key, "sk-script");
    }

    #[test]
    fn zenmux_coding_plan_falls_back_to_provider_credentials() {
        let provider = Provider::with_id(
            "test".to_string(),
            "Test".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://provider.zenmux.example/v1",
                    "ANTHROPIC_AUTH_TOKEN": "sk-provider"
                }
            }),
            None,
        );
        let script = usage_script(Some("zenmux"), Some("https://script.zenmux.example"), None);

        let (base_url, api_key) =
            resolve_coding_plan_credentials(&AppType::Claude, Some(&provider), Some(&script));

        assert_eq!(base_url, "https://provider.zenmux.example/v1");
        assert_eq!(api_key, "sk-provider");
    }
}

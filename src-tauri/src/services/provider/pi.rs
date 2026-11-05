use super::{ProviderService, SwitchResult};
use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta, UsageScript};
use crate::store::AppState;
use indexmap::IndexMap;
use serde_json::Value;

const PI_APP: &str = "pi";

pub(super) fn list(state: &AppState) -> Result<IndexMap<String, Provider>, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(PI_APP));
    match crate::pi_config::read_pi_native_providers() {
        Ok(native) => {
            if let Err(error) = sync_native_locked(state, &native) {
                log::warn!("Failed to sync Pi providers from native config: {error}");
            }
        }
        Err(error) => {
            log::warn!("Failed to read Pi providers; showing saved catalog: {error}");
        }
    }
    state.db.get_all_providers(PI_APP)
}

pub(super) fn import_from_live(state: &AppState) -> Result<usize, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(PI_APP));
    let native = crate::pi_config::read_pi_native_providers()?;
    sync_native_locked(state, &native)
}

pub(super) fn add(state: &AppState, mut provider: Provider) -> Result<bool, AppError> {
    let app_type = AppType::Pi;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    strip_unsupported_pi_metadata(&mut provider);
    ProviderService::validate_provider_settings(&app_type, &provider)?;
    align_native_display_name(&mut provider);
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);

    if state
        .db
        .get_provider_by_id(&provider.id, app_type.as_str())?
        .is_some()
    {
        return Err(AppError::InvalidInput(format!(
            "Pi provider '{}' already exists",
            provider.id
        )));
    }

    state.db.save_provider(app_type.as_str(), &provider)?;
    Ok(true)
}

// Pi、Omp 的用量脚本只更新数据库，避免用旧快照覆盖外部修改过的原生配置。
pub(super) fn update_usage_script(
    state: &AppState,
    app_type: AppType,
    id: &str,
    script: UsageScript,
) -> Result<bool, AppError> {
    if !matches!(app_type, AppType::Pi | AppType::Omp) {
        return Err(AppError::InvalidInput(
            "Usage metadata updates only support Pi and Omp".into(),
        ));
    }
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    super::validate_usage_script(&script)?;

    let mut provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| {
            AppError::InvalidInput(format!("{} provider '{id}' not found", app_type.as_str()))
        })?;
    provider
        .meta
        .get_or_insert_with(ProviderMeta::default)
        .usage_script = Some(script);
    strip_unsupported_pi_metadata(&mut provider);
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);
    state.db.save_provider(app_type.as_str(), &provider)?;
    Ok(true)
}

pub(super) fn update(
    state: &AppState,
    original_id: Option<&str>,
    mut provider: Provider,
) -> Result<bool, AppError> {
    let app_type = AppType::Pi;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let original_id = original_id.unwrap_or(&provider.id).to_string();
    if original_id != provider.id {
        return Err(AppError::InvalidInput(
            "Pi provider keys cannot be renamed".to_string(),
        ));
    }

    state
        .db
        .get_provider_by_id(&original_id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("Pi provider '{original_id}' not found")))?;
    strip_unsupported_pi_metadata(&mut provider);
    ProviderService::validate_provider_settings(&app_type, &provider)?;
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);

    let previous_native =
        crate::pi_config::replace_pi_provider_if_present(&original_id, &provider.settings_config)?;
    if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
        if let Some(previous_native) = previous_native.as_ref() {
            if let Err(rollback) = crate::pi_config::replace_pi_provider(
                &original_id,
                &provider.settings_config,
                previous_native,
            ) {
                return Err(AppError::Config(format!(
                    "failed to save Pi provider: {error}; native rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(true)
}

pub(super) fn delete(state: &AppState, id: &str) -> Result<(), AppError> {
    let app_type = AppType::Pi;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let Some(_) = state.db.get_provider_by_id(id, app_type.as_str())? else {
        return Ok(());
    };
    // Delete is intentionally keyed by provider ID. Once the user confirms
    // deleting the provider itself, supported field edits do not change that
    // intent; the latest native value is retained only for rollback.
    let removed = crate::pi_config::remove_pi_provider(id)?;

    if let Err(error) = state.db.delete_provider(app_type.as_str(), id) {
        if let Some(removed) = removed.as_ref() {
            if let Err(rollback) = crate::pi_config::restore_pi_provider_if_missing(id, removed) {
                return Err(AppError::Config(format!(
                    "failed to delete Pi provider: {error}; native rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
    let app_type = AppType::Pi;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("Pi provider '{id}' not found")))?;
    let Some(removed) = crate::pi_config::remove_pi_provider(id)? else {
        return Ok(());
    };
    let mut synced = provider;
    merge_native_config(&mut synced, removed.clone());
    if let Err(error) = state.db.save_provider(app_type.as_str(), &synced) {
        if let Err(rollback) = crate::pi_config::restore_pi_provider_if_missing(id, &removed) {
            return Err(AppError::Config(format!(
                "failed to preserve Pi provider before removal: {error}; native rollback failed: {rollback}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn enable(state: &AppState, id: &str) -> Result<SwitchResult, AppError> {
    let app_type = AppType::Pi;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("Pi provider '{id}' not found")))?;

    if let Some(native) = crate::pi_config::read_pi_native_provider(id)? {
        let mut synced = provider;
        merge_native_config(&mut synced, native);
        state.db.save_provider(app_type.as_str(), &synced)?;
        return Ok(SwitchResult::default());
    }

    ProviderService::validate_provider_settings(&app_type, &provider)?;
    crate::pi_config::insert_pi_provider(id, &provider.settings_config)?;
    Ok(SwitchResult::default())
}

fn sync_native_locked(
    state: &AppState,
    native: &IndexMap<String, Value>,
) -> Result<usize, AppError> {
    let saved = state.db.get_all_providers(PI_APP)?;
    let mut changed = 0;

    for (id, config) in native {
        let mut provider = saved.get(id).cloned().unwrap_or_else(|| {
            let name = native_provider_name(config).unwrap_or(id).to_string();
            let mut imported = Provider::with_id(id.clone(), name, config.clone(), None);
            imported.category = Some("custom".to_string());
            imported.icon = Some("pi".to_string());
            imported
        });
        let is_new = !saved.contains_key(id);
        let previous_name = provider.name.clone();
        let previous_config = provider.settings_config.clone();
        merge_native_config(&mut provider, config.clone());
        if !is_new && provider.name == previous_name && provider.settings_config == previous_config
        {
            continue;
        }

        state.db.save_provider(PI_APP, &provider)?;
        changed += 1;
    }

    Ok(changed)
}

fn merge_native_config(provider: &mut Provider, config: Value) {
    if let Some(name) = native_provider_name(&config) {
        provider.name = name.to_string();
    }
    provider.settings_config = config;
}

fn native_provider_name(config: &Value) -> Option<&str> {
    config
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
}

fn align_native_display_name(provider: &mut Provider) {
    let Some(config) = provider.settings_config.as_object_mut() else {
        return;
    };
    if config.contains_key("name") {
        config.insert("name".to_string(), Value::String(provider.name.clone()));
    }
}

fn strip_unsupported_pi_metadata(provider: &mut Provider) {
    provider.in_failover_queue = false;
    let Some(meta) = provider.meta.take() else {
        return;
    };
    provider.meta = Some(ProviderMeta {
        usage_script: meta.usage_script,
        is_partner: meta.is_partner,
        partner_promotion_key: meta.partner_promotion_key,
        ..ProviderMeta::default()
    });
}

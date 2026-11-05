use super::{ProviderService, SwitchResult};
use crate::{
    app_config::AppType,
    error::AppError,
    omp_config::{self, Change},
    provider::{Provider, ProviderMeta},
    store::AppState,
};
use indexmap::IndexMap;

fn prepare(provider: &mut Provider) -> Result<(), AppError> {
    omp_config::validate_provider(&provider.id, &provider.settings_config)?;
    provider.in_failover_queue = false;
    provider.meta = provider.meta.take().map(|meta| ProviderMeta {
        usage_script: meta.usage_script,
        is_partner: meta.is_partner,
        partner_promotion_key: meta.partner_promotion_key,
        ..Default::default()
    });
    Ok(())
}

pub(super) fn list(state: &AppState) -> Result<IndexMap<String, Provider>, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app("omp"));
    if let Err(error) = sync_native(state) {
        log::warn!("Failed to read Oh My Pi providers; showing saved catalog: {error}");
    }
    state.db.get_all_providers("omp")
}
fn sync_native(state: &AppState) -> Result<usize, AppError> {
    let native = omp_config::get_providers()?;
    let saved = state.db.get_all_providers("omp")?;
    let mut changed = 0;
    for (id, config) in native {
        let mut provider = saved.get(&id).cloned().unwrap_or_else(|| {
            let name = config
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&id);
            let mut provider = Provider::with_id(id.clone(), name.into(), config.clone(), None);
            provider.category = Some("custom".into());
            provider.icon = Some("omp".into());
            provider
        });
        if saved.contains_key(&id) && provider.settings_config == config {
            continue;
        }
        provider.settings_config = config;
        state.db.save_provider("omp", &provider)?;
        changed += 1;
    }
    Ok(changed)
}
pub(super) fn import_from_live(state: &AppState) -> Result<usize, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app("omp"));
    sync_native(state)
}
pub(super) fn add(state: &AppState, mut provider: Provider) -> Result<bool, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app("omp"));
    prepare(&mut provider)?;
    if state.db.get_provider_by_id(&provider.id, "omp")?.is_some() {
        return Err(AppError::Config("omp.errors.duplicateProvider".into()));
    }
    state.db.save_provider("omp", &provider)?;
    Ok(true)
}
pub(super) fn update(
    state: &AppState,
    original_id: Option<&str>,
    mut provider: Provider,
) -> Result<bool, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app("omp"));
    if original_id.is_some_and(|id| id != provider.id) {
        return Err(AppError::Config("omp.errors.renameProvider".into()));
    }
    state
        .db
        .get_provider_by_id(&provider.id, "omp")?
        .ok_or_else(|| AppError::Config("omp.errors.providerMissing".into()))?;
    prepare(&mut provider)?;
    omp_config::change_and_commit(
        &provider.id,
        Some(&provider.settings_config),
        Change::ReplaceIfPresent,
        |_| {
            state.db.save_provider("omp", &provider)?;
            Ok(true)
        },
    )
}
pub(super) fn delete(state: &AppState, id: &str) -> Result<(), AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app("omp"));
    omp_config::change_and_commit(id, None, Change::Remove, |_| {
        state.db.delete_provider("omp", id)
    })
}
pub(super) fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app("omp"));
    let mut provider = state
        .db
        .get_provider_by_id(id, "omp")?
        .ok_or_else(|| AppError::Config("omp.errors.providerMissing".into()))?;
    omp_config::change_and_commit(id, None, Change::Remove, |old| {
        if let Some(config) = old {
            provider.settings_config = config;
            state.db.save_provider("omp", &provider)?;
        }
        Ok(())
    })
}
pub(super) fn enable(state: &AppState, id: &str) -> Result<SwitchResult, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app("omp"));
    let mut provider = state
        .db
        .get_provider_by_id(id, "omp")?
        .ok_or_else(|| AppError::Config("omp.errors.providerMissing".into()))?;
    if let Some(native) = omp_config::get_providers()?.get(id) {
        provider.settings_config = native.clone();
        state.db.save_provider("omp", &provider)?;
        return Ok(SwitchResult::default());
    }
    ProviderService::validate_provider_settings(&AppType::Omp, &provider)?;
    omp_config::change_and_commit(id, Some(&provider.settings_config), Change::Insert, |_| {
        Ok(SwitchResult::default())
    })
}

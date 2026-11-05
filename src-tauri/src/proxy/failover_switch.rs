//! 故障转移切换模块
//!
//! 处理故障转移成功后的供应商切换逻辑，包括：
//! - 去重控制（避免多个请求同时触发）
//! - 托盘菜单更新
//! - 前端事件发射

use crate::error::AppError;
use std::collections::HashSet;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::RwLock;

/// 故障转移切换管理器
///
/// 负责处理故障转移成功后的供应商切换，确保 UI 能够直观反映当前使用的供应商。
#[derive(Clone, Default)]
pub struct FailoverSwitchManager {
    /// 正在处理中的切换（key = "app_type:provider_id"）
    pending_switches: Arc<RwLock<HashSet<String>>>,
}

impl FailoverSwitchManager {
    pub fn new() -> Self {
        Self {
            pending_switches: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// 尝试执行故障转移切换
    ///
    /// 如果相同的切换已在进行中，则跳过；否则执行切换逻辑。
    ///
    /// # Returns
    /// - `Ok(true)` - 切换成功执行
    /// - `Ok(false)` - 切换已在进行中，跳过
    /// - `Err(e)` - 切换过程中发生错误
    pub async fn try_switch(
        &self,
        app_handle: Option<&tauri::AppHandle>,
        app_type: &str,
        provider_id: &str,
        provider_name: &str,
    ) -> Result<bool, AppError> {
        let switch_key = format!("{app_type}:{provider_id}");

        // 去重检查：如果相同切换已在进行中，跳过
        {
            let mut pending = self.pending_switches.write().await;
            if pending.contains(&switch_key) {
                log::debug!("[Failover] 切换已在进行中，跳过: {app_type} -> {provider_id}");
                return Ok(false);
            }
            pending.insert(switch_key.clone());
        }

        // 执行切换（确保最后清理 pending 标记）
        let result = self
            .do_switch(app_handle, app_type, provider_id, provider_name)
            .await;

        // 清理 pending 标记
        {
            let mut pending = self.pending_switches.write().await;
            pending.remove(&switch_key);
        }

        result
    }

    async fn do_switch(
        &self,
        app_handle: Option<&tauri::AppHandle>,
        app_type: &str,
        provider_id: &str,
        provider_name: &str,
    ) -> Result<bool, AppError> {
        let Ok(app_enum) = app_type.parse::<crate::app_config::AppType>() else {
            return Ok(false);
        };

        log::info!("[FO-001] 切换: {app_type} → {provider_name}");

        let mut switched = false;

        if let Some(app) = app_handle {
            if let Some(app_state) = app.try_state::<crate::store::AppState>() {
                let state = app_state.inner();
                // 只有一份激活指针：故障转移成功就是把这一家设为激活，下一次请求直接用它。
                let active = crate::services::ProviderService::active_provider(&state.db, &app_enum)?;
                if active.as_ref().map(|provider| provider.id.as_str()) == Some(provider_id) {
                    return Ok(false);
                }
                let Some(target) = state
                    .db
                    .get_provider_by_id(provider_id, app_enum.as_str())?
                else {
                    log::warn!("[Failover] 目标供应商 {provider_id} 不在库里，忽略");
                    return Ok(false);
                };

                crate::settings::set_current_provider(&app_enum, Some(provider_id))?;
                state.db.set_current_provider(app_enum.as_str(), provider_id)?;
                state.proxy_service.set_active_target(&app_enum, &target).await;
                switched = true;

                // Linux 重建原生菜单，其它平台通知托盘面板（不挂原生菜单）
                crate::tray::refresh_tray_menu(app);
            }

            // 发射事件到前端
            let event_data = serde_json::json!({
                "appType": app_type,
                "providerId": provider_id,
                "source": "failover"  // 标识来源是故障转移
            });
            if let Err(e) = app.emit("provider-switched", event_data) {
                log::error!("[Failover] 发射事件失败: {e}");
            }
        }

        Ok(switched)
    }
}

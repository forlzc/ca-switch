//! 项目 Profile 编排服务
//!
//! Profile 是**全应用共享的项目实体**（用户拥有的项目就那几个），payload
//! 按 app 分槽存供应商配置快照。快照与应用均**按分组（scope）操作**：
//! 各分组独立指向自己的当前项目、只拍/只应用组内槽位，互不牵连；
//! 重命名/删除作用于共享实体本身。
//! 应用（apply）时复用现有切换原语批量落地：
//! - 供应商：`ProviderService::switch`（内建代理接管热切换与接管下禁切官方）
//!
//! apply 为 best-effort：单项失败收集为 warning 继续，不整体回滚。

use serde::{Deserialize, Serialize};

use crate::app_config::AppType;
use crate::database::Profile;
use crate::error::AppError;
use crate::services::ProviderService;
use crate::store::AppState;

/// Profile 操作的应用分组：当前只有 Claude 一个分组。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileScope {
    Claude,
}

impl ProfileScope {
    /// 全部分组
    pub const ALL: [ProfileScope; 1] = [ProfileScope::Claude];

    pub fn as_str(&self) -> &'static str {
        match self {
            ProfileScope::Claude => "claude",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value {
            "claude" | "anthropic" => Ok(ProfileScope::Claude),
            other => Err(AppError::InvalidInput(format!(
                "Unknown profile scope: {other}"
            ))),
        }
    }

    /// 组内受管应用（快照与 apply 只作用于这些 app 的槽位）
    pub fn apps(&self) -> &'static [AppType] {
        match self {
            ProfileScope::Claude => &[AppType::Claude],
        }
    }

    /// 应用页 → 所属分组
    pub fn for_app(app: &AppType) -> Option<Self> {
        match app {
            AppType::Claude => Some(ProfileScope::Claude),
        }
    }
}

/// 按 app 分槽的载荷容器；字段名与 AppType 的 serde 形式一致
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerApp<T> {
    pub claude: T,
}

impl<T> PerApp<T> {
    pub fn get(&self, app: &AppType) -> Option<&T> {
        match app {
            AppType::Claude => Some(&self.claude),
        }
    }

    pub fn get_mut(&mut self, app: &AppType) -> Option<&mut T> {
        match app {
            AppType::Claude => Some(&mut self.claude),
        }
    }
}

/// Profile 的 JSON 快照结构（与前端 TS 类型严格对应）
///
/// 所有槽位都是 Option：None = 该侧从未拍过快照（应用时不动），
/// 与"拍到的就是空集/无激活项"（Some(空)，应用时清空启用）严格区分——
/// 在 Codex 页选中一个只在 Claude 页建过的项目不能误清 Codex 的启用状态。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfilePayload {
    /// 每 app 的当前供应商 id
    pub providers: PerApp<Option<String>>,
}

impl ProfilePayload {
    /// 用另一份快照覆盖本载荷中某分组的槽位，其余分组原样保留
    /// （"以当前状态更新"只更新发起页所属分组，避免把别的应用
    /// 正处于其他项目的状态串进来）
    pub fn merge_scope_from(&mut self, other: &ProfilePayload, scope: ProfileScope) {
        for app in scope.apps() {
            if let (Some(dst), Some(src)) = (self.providers.get_mut(app), other.providers.get(app))
            {
                *dst = src.clone();
            }
        }
    }

    /// 某分组是否拍过快照（任一槽位非 None 即视为拍过）
    pub fn scope_captured(&self, scope: ProfileScope) -> bool {
        scope
            .apps()
            .iter()
            .any(|app| self.providers.get(app).is_some_and(|s| s.is_some()))
    }
}
pub struct ProfileService;

impl ProfileService {
    /// 抓取分组内应用的当前配置状态生成快照（组外槽位保持默认值）
    pub fn snapshot_current(
        state: &AppState,
        scope: ProfileScope,
    ) -> Result<ProfilePayload, AppError> {
        let mut payload = ProfilePayload::default();

        for app in scope.apps().iter() {
            if let Some(slot) = payload.providers.get_mut(app) {
                *slot = ProviderService::active_provider(&state.db, app)?
                    .map(|provider| provider.id);
            }
        }
        Ok(payload)
    }

    /// 列出所有项目（项目实体全应用共享，current 标记按分组单独读取）
    pub fn list(state: &AppState) -> Result<Vec<Profile>, AppError> {
        state.db.get_all_profiles()
    }

    /// 创建新项目：只拍发起页所属分组的当前状态，其余分组槽位留 None
    /// （其他应用可能正处于别的项目，不能替用户拍进来）
    pub fn create(state: &AppState, name: &str, scope: ProfileScope) -> Result<Profile, AppError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::InvalidInput("Profile name is empty".to_string()));
        }
        let payload = Self::snapshot_current(state, scope)?;
        let now = chrono::Utc::now().timestamp();
        let profile = Profile {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            payload: serde_json::to_string(&payload)
                .map_err(|e| AppError::Config(format!("序列化 profile payload 失败: {e}")))?,
            sort_order: None,
            created_at: Some(now),
            updated_at: Some(now),
        };
        state.db.save_profile(&profile)?;
        Ok(profile)
    }

    /// 更新项目：重命名（作用于共享实体）和/或以当前状态重拍快照
    /// （resnapshot 只覆盖 scope 分组的槽位，其余分组原样保留；
    /// 快照重拍仅由 [`Self::apply`] 切换前的自动保存触发，UI 不再暴露手动入口）
    pub fn update(
        state: &AppState,
        id: &str,
        name: Option<String>,
        resnapshot: bool,
        scope: Option<ProfileScope>,
    ) -> Result<Profile, AppError> {
        let mut profile = state
            .db
            .get_profile(id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Profile not found: {id}")))?;

        if let Some(name) = name {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err(AppError::InvalidInput("Profile name is empty".to_string()));
            }
            profile.name = name;
        }
        if resnapshot {
            let scope = scope.ok_or_else(|| {
                AppError::InvalidInput("Resnapshot requires a profile scope".to_string())
            })?;
            let mut payload: ProfilePayload = serde_json::from_str(&profile.payload)
                .map_err(|e| AppError::Config(format!("解析 profile payload 失败: {e}")))?;
            payload.merge_scope_from(&Self::snapshot_current(state, scope)?, scope);
            profile.payload = serde_json::to_string(&payload)
                .map_err(|e| AppError::Config(format!("序列化 profile payload 失败: {e}")))?;
        }
        profile.updated_at = Some(chrono::Utc::now().timestamp());
        state.db.save_profile(&profile)?;
        Ok(profile)
    }

    /// 删除项目；若删除的是某分组当前激活项目，一并清除该分组的激活标记
    pub fn delete(state: &AppState, id: &str) -> Result<(), AppError> {
        state.db.delete_profile(id)?;
        for scope in ProfileScope::ALL {
            if state.db.get_current_profile_id(scope.as_str())?.as_deref() == Some(id) {
                state.db.set_current_profile_id(scope.as_str(), None)?;
            }
        }
        Ok(())
    }

    /// 应用项目快照（best-effort，返回 warnings）
    ///
    /// 只作用于发起页所属分组内的应用，不碰其他分组的配置与 current 标记。
    /// 该分组从未拍过快照时不改动任何配置，仅标记 current 并返回提示
    /// （下次从该项目切走时，自动保存会补拍该侧快照）。
    ///
    /// **切换前会自动保存旧项目**：若当前分组已绑定到另一个项目，先把当前
    /// 状态写入那个旧项目（仅当前分组槽位），再加载目标项目。这样切走后
    /// 旧项目仍保留离开时的配置，回来时状态一致。自动保存失败时作为 warning
    /// 继续，不阻塞切换。
    ///
    /// 应用指定项目的快照到当前分组内的所有应用。
    ///
    /// 供应商按应用当前的模式切换：直连模式改直连指针，代理模式改代理路由；不再为了
    /// 避开代理先退出代理模式。
    pub fn apply(
        state: &AppState,
        profile_id: &str,
        scope: ProfileScope,
    ) -> Result<Vec<String>, AppError> {
        let mut warnings = Vec::new();

        // 自动保存旧项目当前状态（仅当前分组），失败不阻塞切换
        if let Some(current_id) = state.db.get_current_profile_id(scope.as_str())? {
            if current_id != profile_id {
                if let Err(e) = Self::update(state, &current_id, None, true, Some(scope)) {
                    warnings.push(format!(
                        "autosave profile '{current_id}' before switch failed: {e}"
                    ));
                }
            }
        }

        let profile = state
            .db
            .get_profile(profile_id)?
            .ok_or_else(|| AppError::InvalidInput(format!("Profile not found: {profile_id}")))?;
        let payload: ProfilePayload = serde_json::from_str(&profile.payload)
            .map_err(|e| AppError::Config(format!("解析 profile payload 失败: {e}")))?;

        if !payload.scope_captured(scope) {
            warnings.push(format!(
                "no {} configuration captured in this project yet; marked as current without changes (it will be saved automatically when you switch away)",
                scope.as_str()
            ));
        }

        for app in scope.apps().iter() {
            let app_str = app.as_str();

            // 1. 供应商
            if let Some(Some(target_pid)) = payload.providers.get(app) {
                let providers = state.db.get_all_providers(app_str)?;
                if !providers.contains_key(target_pid) {
                    warnings.push(format!(
                        "[{app_str}] provider '{target_pid}' no longer exists, skipped"
                    ));
                } else {
                    let current = ProviderService::active_provider(&state.db, app)?
                        .map(|provider| provider.id);
                    if current.as_deref() != Some(target_pid.as_str()) {
                        match ProviderService::switch(state, app.clone(), target_pid) {
                            Ok(result) => warnings.extend(result.warnings),
                            Err(e) => warnings.push(format!(
                                "[{app_str}] switch provider '{target_pid}' failed: {e}"
                            )),
                        }
                    }
                }
            }

        }

        state
            .db
            .set_current_profile_id(scope.as_str(), Some(profile_id))?;

        Ok(warnings)
    }
}

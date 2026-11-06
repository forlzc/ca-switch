use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

use crate::config::{copy_file, get_app_config_dir, get_app_config_path, write_json_file};
use crate::error::AppError;
use crate::provider::ProviderManager;

/// 应用类型：收敛为唯一的 API（Anthropic）代理入口
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppType {
    #[serde(alias = "anthropic")]
    Claude,
}

impl AppType {
    pub fn as_str(&self) -> &str {
        match self {
            AppType::Claude => "claude",
        }
    }

    /// 不再存在累加模式应用。
    pub fn is_additive_mode(&self) -> bool {
        false
    }

    /// 唯一的 API 入口支持本地代理。
    pub fn supports_local_proxy(&self) -> bool {
        true
    }

    /// 唯一的 API 入口具备设备级直连 / 路由模式。
    pub fn has_route_mode(&self) -> bool {
        true
    }

    /// Return an iterator over all app types
    pub fn all() -> impl Iterator<Item = AppType> {
        [AppType::Claude].into_iter()
    }
}

impl FromStr for AppType {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_lowercase();
        match normalized.as_str() {
            "claude" | "anthropic" => Ok(AppType::Claude),
            other => Err(AppError::localized(
                "unsupported_app",
                format!("不支持的应用标识: '{other}'。可选值: claude、anthropic。"),
                format!("Unsupported app id: '{other}'. Allowed: claude, anthropic."),
            )),
        }
    }
}

/// 通用配置片段（只有唯一的 API 应用）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CommonConfigSnippets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
}

impl CommonConfigSnippets {
    /// 获取通用配置片段
    pub fn get(&self, _app: &AppType) -> Option<&String> {
        self.claude.as_ref()
    }

    /// 设置通用配置片段
    pub fn set(&mut self, _app: &AppType, snippet: Option<String>) {
        self.claude = snippet;
    }
}

/// 多应用配置结构（向后兼容）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiAppConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    /// 应用管理器（claude/codex）
    #[serde(flatten)]
    pub apps: HashMap<String, ProviderManager>,
    /// 旧版遗留键：MCP / Skills 已移除，这里只为接住旧 config.json 里的同名值。
    /// `apps` 是 flatten 的 HashMap，未声明的键会被当作供应商管理器解析而报错。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<serde_json::Value>,
    /// 通用配置片段（按应用分治）
    #[serde(default)]
    pub common_config_snippets: CommonConfigSnippets,
    /// Claude 通用配置片段（旧字段，用于向后兼容迁移）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_common_config_snippet: Option<String>,
}

fn default_version() -> u32 {
    2
}

impl Default for MultiAppConfig {
    fn default() -> Self {
        let mut apps = HashMap::new();
        apps.insert("claude".to_string(), ProviderManager::default());

        Self {
            version: 2,
            apps,
            mcp: None,
            skills: None,
            common_config_snippets: CommonConfigSnippets::default(),
            claude_common_config_snippet: None,
        }
    }
}

impl MultiAppConfig {
    /// 从文件加载配置（仅支持 v2 结构）
    pub fn load() -> Result<Self, AppError> {
        let config_path = get_app_config_path();

        if !config_path.exists() {
            log::info!("配置文件不存在，创建新的多应用配置");
            let config = Self::default();
            config.save()?;
            return Ok(config);
        }

        // 尝试读取文件
        let content =
            std::fs::read_to_string(&config_path).map_err(|e| AppError::io(&config_path, e))?;

        // 先解析为 Value，以便严格判定是否为 v1 结构；
        // 满足：顶层同时包含 providers(object) + current(string)，且不包含 version/apps/mcp 关键键，即视为 v1
        let value: serde_json::Value =
            serde_json::from_str(&content).map_err(|e| AppError::json(&config_path, e))?;
        let is_v1 = value.as_object().is_some_and(|map| {
            let has_providers = map.get("providers").map(|v| v.is_object()).unwrap_or(false);
            let has_current = map.get("current").map(|v| v.is_string()).unwrap_or(false);
            // v1 的充分必要条件：有 providers 和 current，且 apps 不存在（version/mcp 可能存在但不作为 v2 判据）
            let has_apps = map.contains_key("apps");
            has_providers && has_current && !has_apps
        });
        if is_v1 {
            return Err(AppError::localized(
                "config.unsupported_v1",
                "检测到旧版 v1 配置格式。当前版本已不再支持运行时自动迁移。\n\n解决方案：\n1. 安装 v3.2.x 版本进行一次性自动迁移\n2. 或手动编辑 ~/.cc-switch/config.json，将顶层结构调整为：\n   {\"version\": 2, \"claude\": {...}, \"codex\": {...}, \"mcp\": {...}}\n\n",
                "Detected legacy v1 config. Runtime auto-migration is no longer supported.\n\nSolutions:\n1. Install v3.2.x for one-time auto-migration\n2. Or manually edit ~/.cc-switch/config.json to adjust the top-level structure:\n   {\"version\": 2, \"claude\": {...}, \"codex\": {...}, \"mcp\": {...}}\n\n",
            ));
        }

        // 解析 v2 结构
        let mut config: Self =
            serde_json::from_value(value).map_err(|e| AppError::json(&config_path, e))?;
        let mut updated = false;

        // 确保唯一的应用存在（兼容旧配置文件）
        if !config.apps.contains_key("claude") {
            config
                .apps
                .insert("claude".to_string(), ProviderManager::default());
            updated = true;
        }


        // 迁移通用配置片段：claude_common_config_snippet → common_config_snippets.claude
        if let Some(old_claude_snippet) = config.claude_common_config_snippet.take() {
            log::info!(
                "迁移通用配置：claude_common_config_snippet → common_config_snippets.claude"
            );
            config.common_config_snippets.claude = Some(old_claude_snippet);
            updated = true;
        }

        if updated {
            log::info!("配置结构已更新，保存配置...");
            config.save()?;
        }

        Ok(config)
    }

    /// 保存配置到文件
    pub fn save(&self) -> Result<(), AppError> {
        let config_path = get_app_config_path();
        // 先备份旧版（若存在）到 ~/.cc-switch/config.json.bak，再写入新内容
        if config_path.exists() {
            let backup_path = get_app_config_dir().join("config.json.bak");
            if let Err(e) = copy_file(&config_path, &backup_path) {
                log::warn!("备份 config.json 到 .bak 失败: {e}");
            }
        }

        write_json_file(&config_path, self)?;
        Ok(())
    }

    /// 获取指定应用的管理器
    pub fn get_manager(&self, app: &AppType) -> Option<&ProviderManager> {
        self.apps.get(app.as_str())
    }

    /// 获取指定应用的管理器（可变引用）
    pub fn get_manager_mut(&mut self, app: &AppType) -> Option<&mut ProviderManager> {
        self.apps.get_mut(app.as_str())
    }

    /// 确保应用存在
    pub fn ensure_app(&mut self, app: &AppType) {
        if !self.apps.contains_key(app.as_str()) {
            self.apps
                .insert(app.as_str().to_string(), ProviderManager::default());
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_type_parses_claude_and_anthropic_aliases() {
        assert_eq!("claude".parse::<AppType>().unwrap(), AppType::Claude);
        assert_eq!("anthropic".parse::<AppType>().unwrap(), AppType::Claude);
        assert_eq!(AppType::Claude.as_str(), "claude");
    }


}

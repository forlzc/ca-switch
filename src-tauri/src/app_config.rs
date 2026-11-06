use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

/// Prompt 配置：单客户端维度
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptConfig {
    #[serde(default)]
    pub prompts: HashMap<String, crate::prompt::Prompt>,
}

/// Prompt 根：只维护唯一的 API 应用
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptRoot {
    #[serde(default)]
    pub claude: PromptConfig,
}

use crate::config::{copy_file, get_app_config_dir, get_app_config_path, write_json_file};
use crate::error::AppError;
use crate::prompt_files::prompt_file_path;
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
    /// Prompt 配置（按客户端分治）
    #[serde(default)]
    pub prompts: PromptRoot,
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
            prompts: PromptRoot::default(),
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
            log::info!("配置文件不存在，创建新的多应用配置并自动导入提示词");
            // 使用新的方法，支持自动导入提示词
            let config = Self::default_with_auto_import()?;
            // 立即保存到磁盘
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

        // 对于已经存在的配置文件，如果此前版本还没有 Prompt 功能，
        // 且 prompts 仍然是空的，则尝试自动导入现有提示词文件。
        let imported_prompts = config.maybe_auto_import_prompts_for_existing_config()?;
        if imported_prompts {
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
            log::info!("配置结构已更新（含 Prompt 自动导入），保存配置...");
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

    /// 创建默认配置并自动导入已存在的提示词文件
    fn default_with_auto_import() -> Result<Self, AppError> {
        log::info!("首次启动，创建默认配置并检测提示词文件");

        let mut config = Self::default();

        // 尝试自动导入唯一的 API 应用提示词
        Self::auto_import_prompt_if_exists(&mut config, AppType::Claude)?;

        Ok(config)
    }

    /// 已存在配置文件时的 Prompt 自动导入逻辑
    ///
    /// 适用于「老版本已经生成过 config.json，但当时还没有 Prompt 功能」的升级场景。
    /// 判定规则：
    /// - 仅当所有应用的 prompts 都为空时才尝试导入（避免打扰已经在使用 Prompt 功能的用户）
    /// - 每个应用最多导入一次，对应各自的提示词文件（如 CLAUDE.md/AGENTS.md/GEMINI.md）
    ///
    /// 返回值：
    /// - Ok(true)  表示至少有一个应用成功导入了提示词
    /// - Ok(false) 表示无需导入或未导入任何内容
    fn maybe_auto_import_prompts_for_existing_config(&mut self) -> Result<bool, AppError> {
        // 如果已经有提示词配置，说明用户已经在使用 Prompt 功能，避免再次自动导入
        if !self.prompts.claude.prompts.is_empty() {
            return Ok(false);
        }

        log::info!("检测到已存在配置文件且 Prompt 列表为空，将尝试从现有提示词文件自动导入");

        Self::auto_import_prompt_if_exists(self, AppType::Claude)
    }

    /// 检查并自动导入单个应用的提示词文件
    ///
    /// 返回值：
    /// - Ok(true)  表示成功导入了非空文件
    /// - Ok(false) 表示未导入（文件不存在、内容为空或读取失败）
    fn auto_import_prompt_if_exists(config: &mut Self, app: AppType) -> Result<bool, AppError> {
        let file_path = prompt_file_path(&app)?;

        // 检查文件是否存在
        if !file_path.exists() {
            log::debug!("提示词文件不存在，跳过自动导入: {file_path:?}");
            return Ok(false);
        }

        // 读取文件内容
        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("读取提示词文件失败: {file_path:?}, 错误: {e}");
                return Ok(false); // 失败时不中断，继续处理其他应用
            }
        };

        // 检查内容是否为空
        if content.trim().is_empty() {
            log::debug!("提示词文件内容为空，跳过导入: {file_path:?}");
            return Ok(false);
        }

        log::info!("发现提示词文件，自动导入: {file_path:?}");

        // 创建提示词对象
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(|_| {
                log::warn!("Failed to get system time, using 0 as timestamp");
                0
            });

        let id = format!("auto-imported-{timestamp}");
        let prompt = crate::prompt::Prompt {
            id: id.clone(),
            name: format!(
                "Auto-imported Prompt {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M")
            ),
            content,
            description: Some("Automatically imported on first launch".to_string()),
            enabled: true, // 自动启用
            created_at: Some(timestamp),
            updated_at: Some(timestamp),
        };

        // 插入到唯一的应用配置中
        let prompts = match app {
            AppType::Claude => &mut config.prompts.claude.prompts,
        };

        prompts.insert(id, prompt);

        log::info!("自动导入完成: {}", app.as_str());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::env;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn app_type_parses_claude_and_anthropic_aliases() {
        assert_eq!("claude".parse::<AppType>().unwrap(), AppType::Claude);
        assert_eq!("anthropic".parse::<AppType>().unwrap(), AppType::Claude);
        assert_eq!(AppType::Claude.as_str(), "claude");
    }

    struct TempHome {
        #[allow(dead_code)] // 字段通过 Drop trait 管理临时目录生命周期
        dir: TempDir,
        original_home: Option<String>,
        original_userprofile: Option<String>,
        original_test_home: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("failed to create temp home");
            let original_home = env::var("HOME").ok();
            let original_userprofile = env::var("USERPROFILE").ok();
            let original_test_home = env::var("CC_SWITCH_TEST_HOME").ok();

            env::set_var("HOME", dir.path());
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());

            Self {
                dir,
                original_home,
                original_userprofile,
                original_test_home,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }

            match &self.original_userprofile {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }

            match &self.original_test_home {
                Some(value) => env::set_var("CC_SWITCH_TEST_HOME", value),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    fn write_prompt_file(app: AppType, content: &str) {
        let path = crate::prompt_files::prompt_file_path(&app).expect("prompt path");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, content).expect("write prompt");
    }

    #[test]
    #[serial]
    fn auto_imports_existing_prompt_when_config_missing() {
        let _home = TempHome::new();
        write_prompt_file(AppType::Claude, "# hello");

        let config = MultiAppConfig::load().expect("load config");

        assert_eq!(config.prompts.claude.prompts.len(), 1);
        let prompt = config
            .prompts
            .claude
            .prompts
            .values()
            .next()
            .expect("prompt exists");
        assert!(prompt.enabled);
        assert_eq!(prompt.content, "# hello");

        let config_path = crate::config::get_app_config_path();
        assert!(
            config_path.exists(),
            "auto import should persist config to disk"
        );
    }

    #[test]
    #[serial]
    fn skips_empty_prompt_files_during_import() {
        let _home = TempHome::new();
        write_prompt_file(AppType::Claude, "   \n  ");

        let config = MultiAppConfig::load().expect("load config");
        assert!(
            config.prompts.claude.prompts.is_empty(),
            "empty files must be ignored"
        );
    }

    #[test]
    #[serial]
    fn auto_import_happens_only_once() {
        let _home = TempHome::new();
        write_prompt_file(AppType::Claude, "first version");

        let first = MultiAppConfig::load().expect("load config");
        assert_eq!(first.prompts.claude.prompts.len(), 1);
        let claude_prompt = first
            .prompts
            .claude
            .prompts
            .values()
            .next()
            .expect("prompt exists")
            .content
            .clone();
        assert_eq!(claude_prompt, "first version");

        // 覆盖文件内容，但保留 config.json
        write_prompt_file(AppType::Claude, "second version");
        let second = MultiAppConfig::load().expect("load config again");

        assert_eq!(second.prompts.claude.prompts.len(), 1);
        let prompt = second
            .prompts
            .claude
            .prompts
            .values()
            .next()
            .expect("prompt exists");
        assert_eq!(
            prompt.content, "first version",
            "should not re-import when config already exists"
        );
    }

}

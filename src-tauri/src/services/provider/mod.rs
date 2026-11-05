//! Provider service module
//!
//! Handles provider CRUD operations, switching, and configuration management.

pub(crate) mod agy_direct;
pub(crate) mod claude_official;
mod editor;
mod endpoints;
mod gemini_auth;
pub(crate) mod icon;
mod omp;
mod pi;
mod usage;

use indexmap::IndexMap;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta, UsageResult};
use crate::settings::CustomEndpoint;
use crate::store::AppState;

pub use editor::{EditorSave, EditorView};
use usage::validate_usage_script;

pub fn official_provider_supports_proxy_takeover(app_type: &AppType, provider: &Provider) -> bool {
    matches!(app_type, AppType::Codex)
        && crate::proxy::providers::is_codex_official_provider(provider)
}

/// 新版不再读通用配置片段，但旧设备经云同步拿到新建的行时仍按这个标记合并片段；不写的
/// 话，旧版切到它会把 hooks、MCP 等共享设置整份抹掉。
fn keep_common_config_for_old_versions(provider: &mut Provider) {
    provider
        .meta
        .get_or_insert_with(Default::default)
        .common_config_enabled = Some(true);
}

/// Provider business logic service
pub struct ProviderService;

/// Native provider lists refresh saved rows as well as reading them. Keep the
/// entire snapshot/readback under the same per-app lock as provider mutations,
/// so an in-flight list cannot restore a deleted row or overwrite a newer edit.
fn lock_native_provider_operations(
    state: &AppState,
    app_type: &AppType,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    matches!(app_type, AppType::Mcode | AppType::Kimi | AppType::Dsh).then(|| {
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()))
    })
}

/// Result of a provider switch operation, including any non-fatal warnings
#[derive(Debug, serde::Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SwitchResult {
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    use crate::claude_desktop_config::PROFILE_ID;
    use crate::config::{get_claude_settings_path, read_json_file, write_json_file};
    use crate::database::Database;
    use crate::provider::{
        AuthBinding, AuthBindingSource, ClaudeModelConfig, ProviderMeta, UniversalProvider,
        UsageScript,
    };
    #[cfg(any(target_os = "macos", windows, target_os = "linux"))]
    use crate::provider::{ClaudeDesktopMode, ClaudeDesktopModelRoute};
    use crate::proxy::types::ProxyConfig;
    use crate::store::AppState;
    use serde_json::json;
    use serial_test::serial;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock};
    use tempfile::TempDir;

    struct TempHome {
        #[allow(dead_code)]
        dir: TempDir,
        original_home: Option<String>,
        #[cfg(windows)]
        original_local_app_data: Option<String>,
        original_userprofile: Option<String>,
        original_test_home: Option<String>,
        #[cfg(target_os = "linux")]
        original_xdg_config_home: Option<std::ffi::OsString>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = TempDir::new().expect("failed to create temp home");
            let original_home = env::var("HOME").ok();
            #[cfg(windows)]
            let original_local_app_data = env::var("LOCALAPPDATA").ok();
            let original_userprofile = env::var("USERPROFILE").ok();
            let original_test_home = env::var("CC_SWITCH_TEST_HOME").ok();
            #[cfg(target_os = "linux")]
            let original_xdg_config_home = env::var_os("XDG_CONFIG_HOME");

            env::set_var("HOME", dir.path());
            #[cfg(windows)]
            env::set_var("LOCALAPPDATA", dir.path().join("AppData").join("Local"));
            env::set_var("USERPROFILE", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            // Claude Desktop Linux paths follow XDG_CONFIG_HOME; pin them under the temp home.
            #[cfg(target_os = "linux")]
            env::remove_var("XDG_CONFIG_HOME");

            Self {
                dir,
                original_home,
                #[cfg(windows)]
                original_local_app_data,
                original_userprofile,
                original_test_home,
                #[cfg(target_os = "linux")]
                original_xdg_config_home,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }

            #[cfg(windows)]
            {
                match &self.original_local_app_data {
                    Some(value) => env::set_var("LOCALAPPDATA", value),
                    None => env::remove_var("LOCALAPPDATA"),
                }
            }

            match &self.original_userprofile {
                Some(value) => env::set_var("USERPROFILE", value),
                None => env::remove_var("USERPROFILE"),
            }

            match &self.original_test_home {
                Some(value) => env::set_var("CC_SWITCH_TEST_HOME", value),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }

            #[cfg(target_os = "linux")]
            {
                match &self.original_xdg_config_home {
                    Some(value) => env::set_var("XDG_CONFIG_HOME", value),
                    None => env::remove_var("XDG_CONFIG_HOME"),
                }
            }
        }
    }

    #[cfg(windows)]
    fn claude_desktop_profile_path(home: &Path) -> PathBuf {
        home.join("AppData")
            .join("Local")
            .join("Claude-3p")
            .join("configLibrary")
            .join(format!("{PROFILE_ID}.json"))
    }

    #[cfg(target_os = "macos")]
    fn claude_desktop_profile_path(home: &Path) -> PathBuf {
        home.join("Library")
            .join("Application Support")
            .join("Claude-3p")
            .join("configLibrary")
            .join(format!("{PROFILE_ID}.json"))
    }

    #[cfg(target_os = "linux")]
    fn claude_desktop_profile_path(home: &Path) -> PathBuf {
        home.join(".config")
            .join("Claude-3p")
            .join("configLibrary")
            .join(format!("{PROFILE_ID}.json"))
    }

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    fn with_test_home<T>(test: impl FnOnce(&AppState, &Path) -> T) -> T {
        let _guard = test_guard();
        let temp = tempfile::tempdir().expect("tempdir");
        let old_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");
        let old_home = std::env::var_os("HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", temp.path());
        std::env::set_var("HOME", temp.path());

        let db = Arc::new(Database::memory().expect("in-memory database"));
        let state = AppState::new(db);
        let result = test(&state, temp.path());

        match old_test_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }

        result
    }

    fn codex_settings(base_url: &str, api_key: &str) -> Value {
        json!({
            "auth": {
                "OPENAI_API_KEY": api_key
            },
            "config": format!(
                "model_provider = \"custom\"\n\
                 [model_providers.custom]\n\
                 name = \"custom\"\n\
                 base_url = \"{base_url}\"\n\
                 wire_api = \"chat\"\n"
            )
        })
    }

    #[test]
    #[serial]
    fn codex_editor_metadata_does_not_change_other_app_previews() {
        with_test_home(|state, _home| {
            crate::settings::reload_settings().expect("reload settings");
            let settings = json!({"env": {
                "ANTHROPIC_BASE_URL": "https://relay.example",
                "ANTHROPIC_AUTH_TOKEN": "editor-key",
                "ANTHROPIC_MODEL": "claude-sonnet-5"
            }});
            let expected =
                ProviderService::editor_view(state, AppType::Claude, &settings, None).unwrap();
            let meta = ProviderMeta {
                provider_type: Some("github_copilot".to_string()),
                ..Default::default()
            };
            let actual = ProviderService::editor_view_with_meta(
                state,
                AppType::Claude,
                &settings,
                None,
                Some("absent-provider"),
                Some(&meta),
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        });
    }

    fn usage_script_with_credentials(
        api_key: Option<&str>,
        base_url: Option<&str>,
        template_type: Option<&str>,
    ) -> UsageScript {
        UsageScript {
            enabled: true,
            language: "javascript".to_string(),
            code: "return { remaining: 1, unit: 'USD' };".to_string(),
            timeout: Some(10),
            api_key: api_key.map(str::to_string),
            base_url: base_url.map(str::to_string),
            access_token: None,
            user_id: None,
            template_type: template_type.map(str::to_string),
            auto_query_interval: None,
            coding_plan_provider: None,
            access_key_id: Some("ak-test".to_string()),
            secret_access_key: Some("sk-test".to_string()),
            team_organization_id: None,
            team_project_id: None,
        }
    }

    fn codex_provider_with_usage(
        id: &str,
        base_url: &str,
        api_key: &str,
        usage_api_key: Option<&str>,
        usage_base_url: Option<&str>,
        template_type: Option<&str>,
    ) -> Provider {
        let mut provider = Provider::with_id(
            id.to_string(),
            format!("Provider {id}"),
            codex_settings(base_url, api_key),
            None,
        );
        provider.meta = Some(ProviderMeta {
            usage_script: Some(usage_script_with_credentials(
                usage_api_key,
                usage_base_url,
                template_type,
            )),
            ..Default::default()
        });
        provider
    }

    fn managed_codex_provider(id: &str, account_id: &str) -> Provider {
        let mut provider = Provider::with_id(
            id.to_string(),
            format!("Managed {id}"),
            json!({
                "auth": {},
                "config": ""
            }),
            None,
        );
        provider.category = Some("official".to_string());
        provider.meta = Some(ProviderMeta {
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some(account_id.to_string()),
            }),
            ..Default::default()
        });
        provider
    }

    fn openclaw_provider(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: format!("Provider {id}"),
            settings_config: json!({
                "baseUrl": "https://api.deepseek.com",
                "apiKey": "test-key",
                "api": "openai-completions",
                "models": [],
            }),
            website_url: None,
            category: Some("custom".to_string()),
            created_at: Some(1),
            sort_index: Some(0),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn hermes_provider(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: format!("Provider {id}"),
            settings_config: json!({
                "api": "openai-chat",
                "base_url": "https://api.example.com/v1",
                "api_key": "test-key",
                "models": {
                    "gpt-4o": {
                        "name": "GPT-4o"
                    }
                }
            }),
            website_url: None,
            category: Some("custom".to_string()),
            created_at: Some(1),
            sort_index: Some(0),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn opencode_provider(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: format!("Provider {id}"),
            settings_config: json!({
                "npm": "@ai-sdk/openai-compatible",
                "name": format!("Provider {id}"),
                "options": {
                    "baseURL": "https://api.example.com/v1",
                    "apiKey": "test-key"
                },
                "models": {
                    "gpt-4o": {
                        "name": "GPT-4o"
                    }
                }
            }),
            website_url: None,
            category: Some("custom".to_string()),
            created_at: Some(1),
            sort_index: Some(0),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn opencode_omo_provider(id: &str, category: &str) -> Provider {
        let mut settings = serde_json::Map::new();
        settings.insert(
            "agents".to_string(),
            json!({
                "writer": {
                    "model": "gpt-4o-mini"
                }
            }),
        );
        if category == "omo" {
            settings.insert(
                "categories".to_string(),
                json!({
                    "default": ["writer"]
                }),
            );
        }
        settings.insert(
            "otherFields".to_string(),
            json!({
                "theme": "dark"
            }),
        );

        Provider {
            id: id.to_string(),
            name: format!("Provider {id}"),
            settings_config: Value::Object(settings),
            website_url: None,
            category: Some(category.to_string()),
            created_at: Some(1),
            sort_index: Some(0),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
            in_failover_queue: false,
        }
    }

    fn omo_config_path(home: &Path, category: &str) -> PathBuf {
        home.join(".config").join("opencode").join(match category {
            "omo" => crate::services::omo::STANDARD.preferred_filename,
            "omo-slim" => crate::services::omo::SLIM.preferred_filename,
            other => panic!("unexpected OMO category in test: {other}"),
        })
    }

    #[test]
    #[serial]
    fn add_clears_usage_credentials_that_match_provider_config() {
        with_test_home(|state, _| {
            let provider = codex_provider_with_usage(
                "codex-a",
                "https://api.a.example/v1/",
                "sk-a",
                Some(" sk-a "),
                Some(" https://api.a.example/v1/ "),
                None,
            );

            ProviderService::add(state, AppType::Codex, provider).expect("add provider");

            let saved = state
                .db
                .get_provider_by_id("codex-a", AppType::Codex.as_str())
                .expect("query saved provider")
                .expect("saved provider should exist");
            let script = saved
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .expect("usage script should remain");

            assert_eq!(script.api_key, None);
            assert_eq!(script.base_url, None);
        });
    }

    #[test]
    #[serial]
    fn update_preserves_usage_credentials_that_only_match_previous_config() {
        with_test_home(|state, _| {
            let provider = codex_provider_with_usage(
                "codex-usage-old",
                "https://api.a.example/v1/",
                "sk-a",
                Some("sk-a"),
                Some("https://api.a.example/v1/"),
                None,
            );
            state
                .db
                .save_provider(AppType::Codex.as_str(), &provider)
                .expect("seed provider with explicit usage credentials");

            let mut updated = provider.clone();
            updated.settings_config = codex_settings("https://api.b.example/v1/", "sk-b");

            ProviderService::update(state, AppType::Codex, None, updated)
                .expect("update provider main credentials");

            let saved = state
                .db
                .get_provider_by_id("codex-usage-old", AppType::Codex.as_str())
                .expect("query updated provider")
                .expect("updated provider should exist");
            let script = saved
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .expect("usage script should remain");

            assert_eq!(script.api_key.as_deref(), Some("sk-a"));
            assert_eq!(
                script.base_url.as_deref(),
                Some("https://api.a.example/v1/")
            );
            assert_eq!(
                saved.resolve_usage_credentials(&AppType::Codex),
                ("https://api.b.example/v1".to_string(), "sk-b".to_string())
            );
        });
    }

    #[test]
    #[serial]
    fn copied_provider_uses_edited_credentials_after_add_clears_mirrored_usage_credentials() {
        with_test_home(|state, _| {
            let copied_provider = codex_provider_with_usage(
                "codex-copy",
                "https://api.a.example/v1/",
                "sk-a",
                Some("sk-a"),
                Some("https://api.a.example/v1/"),
                None,
            );

            ProviderService::add(state, AppType::Codex, copied_provider)
                .expect("add copied provider");

            let saved_after_add = state
                .db
                .get_provider_by_id("codex-copy", AppType::Codex.as_str())
                .expect("query copied provider")
                .expect("copied provider should exist");
            let script_after_add = saved_after_add
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .expect("usage script should remain");
            assert_eq!(script_after_add.api_key, None);
            assert_eq!(script_after_add.base_url, None);

            let mut edited_provider = saved_after_add.clone();
            edited_provider.settings_config = codex_settings("https://api.b.example/v1/", "sk-b");

            ProviderService::update(state, AppType::Codex, None, edited_provider)
                .expect("edit copied provider credentials");

            let saved_after_update = state
                .db
                .get_provider_by_id("codex-copy", AppType::Codex.as_str())
                .expect("query edited provider")
                .expect("edited provider should exist");
            let script_after_update = saved_after_update
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .expect("usage script should remain");

            assert_eq!(script_after_update.api_key, None);
            assert_eq!(script_after_update.base_url, None);
            assert_eq!(
                saved_after_update.resolve_usage_credentials(&AppType::Codex),
                ("https://api.b.example/v1".to_string(), "sk-b".to_string())
            );
        });
    }

    #[test]
    #[serial]
    fn update_clears_usage_credentials_that_match_current_config() {
        with_test_home(|state, _| {
            let provider = codex_provider_with_usage(
                "codex-current",
                "https://api.a.example/v1",
                "sk-a",
                Some("sk-usage"),
                Some("https://usage.example/api"),
                None,
            );
            state
                .db
                .save_provider(AppType::Codex.as_str(), &provider)
                .expect("seed provider with distinct usage credentials");

            let mut updated = provider.clone();
            updated.settings_config = codex_settings("https://api.b.example/v1/", "sk-b");
            updated.meta = Some(ProviderMeta {
                usage_script: Some(usage_script_with_credentials(
                    Some(" sk-b "),
                    Some(" https://api.b.example/v1/ "),
                    None,
                )),
                ..Default::default()
            });

            ProviderService::update(state, AppType::Codex, None, updated)
                .expect("update provider with redundant usage credentials");

            let saved = state
                .db
                .get_provider_by_id("codex-current", AppType::Codex.as_str())
                .expect("query updated provider")
                .expect("updated provider should exist");
            let script = saved
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .expect("usage script should remain");

            assert_eq!(script.api_key, None);
            assert_eq!(script.base_url, None);
        });
    }

    #[test]
    #[serial]
    fn add_preserves_distinct_usage_credentials() {
        with_test_home(|state, _| {
            let provider = codex_provider_with_usage(
                "codex-distinct",
                "https://api.main.example/v1",
                "sk-main",
                Some("sk-usage"),
                Some("https://usage.example/api"),
                None,
            );

            ProviderService::add(state, AppType::Codex, provider).expect("add provider");

            let saved = state
                .db
                .get_provider_by_id("codex-distinct", AppType::Codex.as_str())
                .expect("query saved provider")
                .expect("saved provider should exist");
            let script = saved
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .expect("usage script should remain");

            assert_eq!(script.api_key.as_deref(), Some("sk-usage"));
            assert_eq!(
                script.base_url.as_deref(),
                Some("https://usage.example/api")
            );
        });
    }

    #[test]
    #[serial]
    fn add_does_not_clear_token_plan_credentials() {
        with_test_home(|state, _| {
            let provider = codex_provider_with_usage(
                "codex-token-plan",
                "https://api.plan.example/v1",
                "sk-plan",
                Some("sk-plan"),
                Some("https://api.plan.example/v1"),
                Some("token_plan"),
            );

            ProviderService::add(state, AppType::Codex, provider).expect("add provider");

            let saved = state
                .db
                .get_provider_by_id("codex-token-plan", AppType::Codex.as_str())
                .expect("query saved provider")
                .expect("saved provider should exist");
            let script = saved
                .meta
                .as_ref()
                .and_then(|meta| meta.usage_script.as_ref())
                .expect("usage script should remain");

            assert_eq!(script.api_key.as_deref(), Some("sk-plan"));
            assert_eq!(
                script.base_url.as_deref(),
                Some("https://api.plan.example/v1")
            );
            assert_eq!(script.access_key_id.as_deref(), Some("ak-test"));
            assert_eq!(script.secret_access_key.as_deref(), Some("sk-test"));
        });
    }

    #[test]
    fn validate_provider_settings_rejects_missing_auth() {
        let provider = Provider::with_id(
            "codex".into(),
            "Codex".into(),
            json!({ "config": "base_url = \"https://example.com\"" }),
            None,
        );
        let err = ProviderService::validate_provider_settings(&AppType::Codex, &provider)
            .expect_err("missing auth should be rejected");
        assert!(
            err.to_string().contains("auth"),
            "expected auth error, got {err:?}"
        );
    }

    #[test]
    #[serial]
    fn add_accepts_multiple_unbound_codex_official_cards() {
        with_test_home(|state, _| {
            crate::settings::reload_settings().expect("reload settings");
            state
                .db
                .init_default_official_providers()
                .expect("seed official providers");
            let fixed_id = crate::database::CODEX_OFFICIAL_PROVIDER_ID;
            state
                .db
                .set_current_provider(AppType::Codex.as_str(), fixed_id)
                .expect("set database current");
            crate::settings::set_current_provider(&AppType::Codex, Some(fixed_id))
                .expect("set local current");

            for id in ["follow-login-a", "follow-login-b"] {
                let mut provider = Provider::with_id(
                    id.to_string(),
                    id.to_string(),
                    json!({ "auth": {}, "config": "" }),
                    None,
                );
                provider.category = Some("official".to_string());
                ProviderService::add(state, AppType::Codex, provider)
                    .expect("add unbound Official card");
            }

            let providers = state
                .db
                .get_all_providers(AppType::Codex.as_str())
                .expect("read providers");
            assert!(providers.contains_key("follow-login-a"));
            assert!(providers.contains_key("follow-login-b"));
        });
    }


    #[test]
    fn extract_gemini_common_config_strips_credentials_keeps_shareable() {
        // Gemini 的共享片段会被 deep-merge 回**其它** Gemini 供应商的 env
        // (旧版切换时合并片段；新版片段冻结只给旧版读)，因此任何凭据都不得进入片段。
        // 之前这里只硬编码跳过 GEMINI_API_KEY/GOOGLE_GEMINI_BASE_URL，而
        // GOOGLE_API_KEY 是 provider.rs 认可的一等 Gemini 凭据 → 会泄露到别的供应商。
        let settings = json!({
            "env": {
                "GEMINI_API_KEY": "g-gem",
                "GOOGLE_API_KEY": "g-legacy-real-key",
                "GOOGLE_GEMINI_BASE_URL": "https://gemini.example",
                "GOOGLE_APPLICATION_CREDENTIALS": "/path/creds.json",
                "SOME_PROXY_AUTH_TOKEN": "tok-proxy",
                // 关键字段归供应商：片段已冻结，收进去会从行里剥掉、再也写不回 live
                "GEMINI_MODEL": "gemini-2.5-pro",
                "GOOGLE_GENAI_USE_VERTEXAI": "true",
                // 可共享的非机密配置必须保留
                "GEMINI_TIMEOUT_MS": "30000"
            }
        });

        let snippet =
            ProviderService::extract_gemini_common_config(&settings).expect("extract should work");
        let value: Value = serde_json::from_str(&snippet).expect("snippet is valid JSON");

        for leaked in [
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "SOME_PROXY_AUTH_TOKEN",
            "GEMINI_MODEL",
            "GOOGLE_GENAI_USE_VERTEXAI",
        ] {
            assert!(
                value.get(leaked).is_none(),
                "{leaked} must not go into the shared Gemini snippet"
            );
        }
        assert_eq!(
            value.get("GEMINI_TIMEOUT_MS").and_then(|v| v.as_str()),
            Some("30000"),
            "shareable non-secret config must be preserved"
        );
    }

    /// 造一个「已被污染」的现场：片段里带 A 账号的凭据 + 一个合法可共享键。
    #[test]
    fn sensitive_key_matcher_covers_common_credential_namings() {
        for key in [
            // 裸 `_KEY`：最常见的写法，却曾被"只枚举 `_API_KEY` 这些子类"漏在外面
            "OPENAI_KEY",
            "GROQ_KEY",
            "XAI_KEY",
            // 不带分隔符的复合写法
            "VOLC_ACCESSKEY",
            "ALIYUN_SECRETKEY",
            "SOME_APITOKEN",
            // personal access token：既不含 TOKEN 也不含 KEY
            "GITHUB_PAT",
            "gitlab_pat",
            // 口令类缩写
            "MYSQL_PWD",
            "DB_PASS",
            "GPG_PASSPHRASE",
            "AWS_CREDS",
            // 发往上游的自定义请求头、Cookie、Authorization
            "ANTHROPIC_CUSTOM_HEADERS",
            "GEMINI_CLI_CUSTOM_HEADERS",
            "headers",
            "UPSTREAM_COOKIE",
            "PROXY_AUTHORIZATION",
        ] {
            assert!(
                ProviderService::is_sensitive_config_key(key),
                "{key} must be treated as a credential"
            );
        }

        // 后缀必须带下划线，不能把正常配置一起卷进来
        for key in [
            "PATH",
            "OLDPWD",
            "GEMINI_COMPAT",
            "SSL_BYPASS",
            "GEMINI_TIMEOUT_MS",
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            // 名字带 HEADER 但不是发往上游的凭据：普通开关、用户自己的遥测端点
            "CLAUDE_CODE_ATTRIBUTION_HEADER",
            "OTEL_EXPORTER_OTLP_HEADERS",
        ] {
            assert!(
                !ProviderService::is_sensitive_config_key(key),
                "{key} is ordinary shareable config and must not be stripped"
            );
        }
    }

    /// 测试夹具：按键排序写一份 `~/.gemini/.env`。
    fn write_gemini_env(map: &HashMap<String, String>) -> Result<(), AppError> {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        let text = keys
            .into_iter()
            .map(|key| format!("{key}={}", map[key]))
            .collect::<Vec<_>>()
            .join("\n");
        crate::gemini_config::write_gemini_env_text_atomic(&text)
    }

    fn seed_leaked_gemini_state(db: &Arc<Database>) {
        db.set_config_snippet(
            "gemini",
            Some(
                json!({
                    "GOOGLE_API_KEY": "key-A-leaked",
                    "SOME_PROXY_AUTH_TOKEN": "tok-A-leaked",
                    "GEMINI_TIMEOUT_MS": "30000"
                })
                .to_string(),
            ),
        )
        .expect("seed snippet");

        // 受害者 B：泄漏的密钥已经被合并进它的 env
        let victim = Provider::with_id(
            "b".into(),
            "Relay B".into(),
            json!({ "env": {
                "GOOGLE_GEMINI_BASE_URL": "https://relay-b.example",
                "GOOGLE_API_KEY": "key-A-leaked",
                "GEMINI_TIMEOUT_MS": "30000"
            }}),
            None,
        );
        db.save_provider("gemini", &victim).expect("save victim");

        // 供应商 C：自己写了同名键但值不同，不能被误删
        let unrelated = Provider::with_id(
            "c".into(),
            "Own Key C".into(),
            json!({ "env": {
                "GOOGLE_GEMINI_BASE_URL": "https://c.example",
                "GOOGLE_API_KEY": "key-C-owned"
            }}),
            None,
        );
        db.save_provider("gemini", &unrelated).expect("save c");
    }

    #[test]
    fn extract_claude_common_config_strips_all_credentials_keeps_shareable() {
        // env 混入多种凭据（Anthropic/OpenRouter/Google/OpenAI/Gemini + AWS/Vertex）
        // 与可共享配置；顶层混入非标准的 apiKey/api_key 凭据与正常设置。
        let settings = json!({
            "env": {
                "ANTHROPIC_API_KEY": "sk-ant",
                "ANTHROPIC_AUTH_TOKEN": "tok-ant",
                "OPENROUTER_API_KEY": "sk-or",
                "GOOGLE_API_KEY": "g-key",
                "OPENAI_API_KEY": "sk-oai",
                "GEMINI_API_KEY": "g-gem",
                "AWS_ACCESS_KEY_ID": "AKIA",
                "AWS_SECRET_ACCESS_KEY": "secret",
                "AWS_SESSION_TOKEN": "sess",
                "GOOGLE_APPLICATION_CREDENTIALS": "/path/creds.json",
                "AWS_BEARER_TOKEN_BEDROCK": "bedrock-tok",
                "ANTHROPIC_BASE_URL": "https://example.com",
                "ANTHROPIC_MODEL": "claude-x",
                "CLAUDE_CODE_SUBAGENT_MODEL": "gpt-5.4-mini",
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "400000",
                "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "400000",
                // 可共享、非机密配置（复数 _TOKENS 不应被误剥）
                "ENABLE_TOOL_SEARCH": "true",
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS": "8192"
            },
            "apiKey": "sk-top",
            "api_key": "sk-top2",
            "theme": "dark",
            "includeCoAuthoredBy": false
        });

        let snippet = ProviderService::extract_claude_common_config(&settings)
            .expect("extract should succeed");
        let value: Value = serde_json::from_str(&snippet).expect("snippet is valid JSON");

        // 所有凭据都不得出现在共享片段里
        let env = value.get("env");
        for leaked in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "OPENROUTER_API_KEY",
            "GOOGLE_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "AWS_BEARER_TOKEN_BEDROCK",
        ] {
            assert!(
                env.and_then(|e| e.get(leaked)).is_none(),
                "credential {leaked} must not leak into common config"
            );
        }
        assert!(
            value.get("apiKey").is_none() && value.get("api_key").is_none(),
            "top-level credentials must be stripped"
        );

        // 端点/模型（provider-specific 非机密）也应剥掉
        assert!(env.and_then(|e| e.get("ANTHROPIC_BASE_URL")).is_none());
        assert!(env.and_then(|e| e.get("ANTHROPIC_MODEL")).is_none());
        assert!(env
            .and_then(|e| e.get("CLAUDE_CODE_SUBAGENT_MODEL"))
            .is_none());
        assert!(env
            .and_then(|e| e.get("CLAUDE_CODE_MAX_CONTEXT_TOKENS"))
            .is_none());
        assert!(env
            .and_then(|e| e.get("CLAUDE_CODE_AUTO_COMPACT_WINDOW"))
            .is_none());

        // 可共享的非机密配置必须保留（含复数 _TOKENS 不被误剥）
        assert_eq!(
            env.and_then(|e| e.get("ENABLE_TOOL_SEARCH"))
                .and_then(|v| v.as_str()),
            Some("true")
        );
        assert_eq!(
            env.and_then(|e| e.get("CLAUDE_CODE_MAX_OUTPUT_TOKENS"))
                .and_then(|v| v.as_str()),
            Some("8192")
        );
        assert_eq!(value.get("theme").and_then(|v| v.as_str()), Some("dark"));
        assert_eq!(value.get("includeCoAuthoredBy"), Some(&json!(false)));
    }

    /// 关键字段（协议选择器、Bedrock/Vertex 区域、`/model` 的选择等）不进共享片段；
    /// 同在 `CLAUDE_CODE_USE_` 前缀下、与供应商无关的开关照常共享。
    #[test]
    fn extract_claude_common_config_keeps_key_fields_per_provider() {
        let settings = json!({
            "env": {
                "CLAUDE_CODE_USE_BEDROCK": "1",
                "CLAUDE_CODE_USE_VERTEX": "1",
                "CLAUDE_CODE_SKIP_BEDROCK_AUTH": "1",
                "AWS_REGION": "us-west-2",
                "AWS_PROFILE": "work",
                "CLOUD_ML_REGION": "us-east5",
                "VERTEX_REGION_CLAUDE_4_0_OPUS": "europe-west1",
                "ANTHROPIC_VERTEX_PROJECT_ID": "my-project",
                "ANTHROPIC_SMALL_FAST_MODEL": "haiku",
                "CLAUDE_CODE_SUBAGENT_MODEL_FORCE": "1",
                "CLAUDE_CODE_USE_POWERSHELL_TOOL": "1",
                "CLAUDE_CODE_ATTRIBUTION_HEADER": "0",
                "DISABLE_TELEMETRY": "1"
            },
            "model": "opus",
            "fallbackModel": "sonnet",
            "apiKeyHelper": "~/bin/key.sh",
            "awsAuthRefresh": "aws sso login",
            "hooks": { "Stop": [] },
            "theme": "dark"
        });

        let snippet = ProviderService::extract_claude_common_config(&settings)
            .expect("extract should succeed");
        let value: Value = serde_json::from_str(&snippet).expect("snippet is valid JSON");

        assert_eq!(
            value,
            json!({
                "env": {
                    "CLAUDE_CODE_USE_POWERSHELL_TOOL": "1",
                    "CLAUDE_CODE_ATTRIBUTION_HEADER": "0",
                    "DISABLE_TELEMETRY": "1"
                },
                "hooks": { "Stop": [] },
                "theme": "dark"
            })
        );
        assert!(
            snippet.find("CLAUDE_CODE_USE_POWERSHELL_TOOL") < snippet.find("DISABLE_TELEMETRY"),
            "removing keys must not reorder the ones that stay: {snippet}"
        );
    }

    /// Regression for issue #4272: Fable tier env keys must not enter the shared
    /// Claude common-config snippet (same class as haiku/sonnet/opus model pins).
    #[test]
    fn extract_claude_common_config_strips_fable_model_env_keys() {
        let settings = json!({
            "env": {
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": "haiku-mapped",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME": "Haiku Mapped",
                "ANTHROPIC_DEFAULT_SONNET_MODEL": "sonnet-mapped[1M]",
                "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Sonnet Mapped",
                "ANTHROPIC_DEFAULT_OPUS_MODEL": "opus-mapped[1M]",
                "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME": "Opus Mapped",
                "ANTHROPIC_DEFAULT_FABLE_MODEL": "deepseek-v4-flash[1M]",
                "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME": "deepseek-v4-flash",
                "ANTHROPIC_MODEL": "default-mapped",
                "ENABLE_TOOL_SEARCH": "true"
            },
            "theme": "dark"
        });

        let snippet = ProviderService::extract_claude_common_config(&settings)
            .expect("extract should succeed");
        let value: Value = serde_json::from_str(&snippet).expect("snippet is valid JSON");
        let env = value.get("env");

        for stripped in [
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
            "ANTHROPIC_DEFAULT_FABLE_MODEL",
            "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
            "ANTHROPIC_MODEL",
        ] {
            assert!(
                env.and_then(|e| e.get(stripped)).is_none(),
                "provider-specific model key {stripped} must not enter common config"
            );
        }

        assert_eq!(
            env.and_then(|e| e.get("ENABLE_TOOL_SEARCH"))
                .and_then(|v| v.as_str()),
            Some("true")
        );
        assert_eq!(value.get("theme").and_then(|v| v.as_str()), Some("dark"));
    }

    #[test]
    fn extract_credentials_returns_expected_values() {
        let provider = Provider::with_id(
            "claude".into(),
            "Claude".into(),
            json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "token",
                    "ANTHROPIC_BASE_URL": "https://claude.example"
                }
            }),
            None,
        );
        let (api_key, base_url) =
            ProviderService::extract_credentials(&provider, &AppType::Claude).unwrap();
        assert_eq!(api_key, "token");
        assert_eq!(base_url, "https://claude.example");
    }

    #[test]
    fn extract_codex_common_config_strips_provider_fields_and_injected_artifacts() {
        // 顶层 experimental_bearer_token 模拟无活跃路由时的 fallback 注入；
        // web_search = "disabled" 是 cc-switch 对黑名单网关注入的哨兵；
        // 顶层 wire_api 模拟无 model_provider 时的 fallback 写法；
        // [mcp.servers] 是历史错误格式，sync_all_enabled 清不掉它。
        let config_toml = r#"model_provider = "azure"
model = "gpt-4"
wire_api = "chat"
disable_response_storage = true
model_reasoning_effort = "high"
approval_policy = "on-request"
experimental_bearer_token = "sk-live-secret"
model_catalog_json = "cc-switch-model-catalog.json"
web_search = "disabled"

[agents]
default_subagent_model = "gpt-4-mini"
max_threads = 4

[model_providers.azure]
name = "Azure OpenAI"
base_url = "https://azure.example/v1"
wire_api = "responses"

[mcp_servers.my_server]
base_url = "http://localhost:8080"

[mcp.servers.legacy_server]
command = "legacy-cmd"
"#;

        let settings = json!({ "config": config_toml });
        let extracted = ProviderService::extract_codex_common_config(&settings)
            .expect("extract_codex_common_config should succeed");

        assert!(
            !extracted
                .lines()
                .any(|line| line.trim_start().starts_with("model_provider")),
            "should remove top-level model_provider"
        );
        assert!(
            !extracted
                .lines()
                .any(|line| line.trim_start().starts_with("model =")),
            "should remove top-level model"
        );
        assert!(
            !extracted.contains("[model_providers"),
            "should remove entire model_providers table"
        );
        // MCP 归 DB mcp_servers 表所有，不得进共享片段（含历史错误格式 [mcp.servers]）
        assert!(
            !extracted.contains("mcp_servers") && !extracted.contains("http://localhost:8080"),
            "should strip mcp_servers from the shared snippet, got: {extracted}"
        );
        assert!(
            !extracted.contains("[mcp") && !extracted.contains("legacy-cmd"),
            "should strip the legacy [mcp.servers] form from the shared snippet, got: {extracted}"
        );
        // 顶层 wire_api 是供应商路由语义（model_providers 整表已剥，
        // 剩余任何 wire_api 都意味着泄漏）
        assert!(
            !extracted.contains("wire_api"),
            "should strip top-level wire_api from the shared snippet, got: {extracted}"
        );
        // 注入产物不得进共享片段（bearer token 泄漏为密钥级问题）
        assert!(
            !extracted.contains("experimental_bearer_token")
                && !extracted.contains("sk-live-secret"),
            "should strip top-level fallback bearer token, got: {extracted}"
        );
        assert!(
            !extracted.contains("model_catalog_json"),
            "should strip catalog projection pointer, got: {extracted}"
        );
        assert!(
            !extracted.contains("web_search"),
            "should strip the cc-switch web_search disabled sentinel, got: {extracted}"
        );
        // 关键字段归供应商（片段已冻结，收进去会从行里剥掉、再也写不回 live）
        for key in [
            "disable_response_storage",
            "model_reasoning_effort",
            "default_subagent_model",
        ] {
            assert!(
                !extracted.contains(key),
                "key field {key} must stay with the provider, got: {extracted}"
            );
        }
        // 真正可共享的键保留
        assert!(
            extracted.contains("approval_policy = \"on-request\"")
                && extracted.contains("max_threads = 4"),
            "shareable keys must survive extraction, got: {extracted}"
        );
    }

    #[test]
    fn extract_codex_common_config_keeps_user_set_web_search() {
        let config_toml = "web_search = \"enabled\"\ndisable_response_storage = true\n";
        let settings = json!({ "config": config_toml });
        let extracted = ProviderService::extract_codex_common_config(&settings)
            .expect("extract should succeed");
        assert!(
            extracted.contains("web_search = \"enabled\""),
            "a user-set web_search value is a shareable preference, got: {extracted}"
        );
    }




    async fn remove_codex_account_leaving_dangling_bindings(
        state: &AppState,
        account_id: &str,
    ) -> Result<(), String> {
        let _switch_guard = state
            .proxy_service
            .lock_switch_for_app(AppType::Codex.as_str())
            .await;
        state
            .codex_oauth_manager
            .remove_account(account_id)
            .await
            .map_err(|error| error.to_string())
    }

    fn seed_codex_recovery_accounts(state: &AppState) {
        tauri::async_runtime::block_on(async {
            for (id, user) in [("acct-old", "old-user"), ("acct-new", "new-user")] {
                state
                    .codex_oauth_manager
                    .add_test_account_with_user_identity(id, "access", user)
                    .await
                    .expect("seed account");
            }
        });
    }

    #[test]
    #[serial]
    fn codex_account_removal_and_logout_unbind_current_and_inactive_cards() {
        for logout_all in [false, true] {
            with_test_home(|state, _| {
                crate::settings::reload_settings().unwrap();
                seed_codex_recovery_accounts(state);
                let current = managed_codex_provider("current", "acct-old");
                let mut legacy = managed_codex_provider("legacy", "acct-old");
                legacy.category = None;
                legacy.notes = Some("keep notes".into());
                legacy.settings_config["auth"] =
                    json!({"tokens": {"refresh_token": "stale-snapshot"}});
                legacy.settings_config["model"] = json!("keep-model");
                let other = managed_codex_provider("other", "acct-new");
                // 从另一台设备同步来的行：绑的是那台设备的本地账号，本机删号或登出都不能动它，
                // 否则解绑会随云同步回去，弄坏那台设备上正常的绑定。
                let foreign = managed_codex_provider("foreign", "acct-other-device");
                let mut unrelated = managed_codex_provider("unrelated", "acct-old");
                unrelated
                    .meta
                    .as_mut()
                    .unwrap()
                    .auth_binding
                    .as_mut()
                    .unwrap()
                    .source = AuthBindingSource::ProviderConfig;
                for provider in [&current, &legacy, &other, &foreign, &unrelated] {
                    state.db.save_provider("codex", provider).unwrap();
                }
                state
                    .db
                    .conn
                    .lock()
                    .unwrap()
                    .execute(
                        "UPDATE providers SET meta = json_set(meta, '$.futureField', 'keep') WHERE id = 'legacy'",
                        [],
                    )
                    .unwrap();
                ProviderService::switch(state, AppType::Codex, &current.id).unwrap();
                if logout_all {
                    tauri::async_runtime::block_on(
                        crate::commands::logout_codex_oauth_with_switch_lock(state),
                    )
                    .unwrap();
                } else {
                    tauri::async_runtime::block_on(
                        crate::commands::remove_codex_oauth_account_with_switch_lock(
                            state, "acct-old",
                        ),
                    )
                    .unwrap();
                }
                for id in ["current", "legacy"] {
                    let saved = state.db.get_provider_by_id(id, "codex").unwrap().unwrap();
                    assert!(saved.meta.unwrap().auth_binding.is_none());
                    assert_eq!(saved.settings_config["auth"], json!({}));
                    assert_eq!(saved.category.as_deref(), Some("official"));
                }
                let saved_legacy = state
                    .db
                    .get_provider_by_id("legacy", "codex")
                    .unwrap()
                    .unwrap();
                assert_eq!(saved_legacy.notes, legacy.notes);
                assert_eq!(saved_legacy.settings_config["model"], "keep-model");
                let future: String = state
                    .db
                    .conn
                    .lock()
                    .unwrap()
                    .query_row(
                        "SELECT json_extract(meta, '$.futureField') FROM providers WHERE id = 'legacy'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(future, "keep");
                let bound_to = |id: &str, app: &str| {
                    state
                        .db
                        .get_provider_by_id(id, app)
                        .unwrap()
                        .unwrap()
                        .meta
                        .unwrap()
                        .managed_account_id_for("codex_oauth")
                };
                assert_eq!(bound_to("other", "codex").is_none(), logout_all);
                assert_eq!(
                    bound_to("foreign", "codex").as_deref(),
                    Some("acct-other-device")
                );
                assert!(state
                    .db
                    .get_provider_by_id("unrelated", "codex")
                    .unwrap()
                    .unwrap()
                    .meta
                    .unwrap()
                    .auth_binding
                    .is_some());
                assert_eq!(
                    crate::settings::get_current_provider(&AppType::Codex).as_deref(),
                    Some("current")
                );
                assert!(!crate::codex_config::get_codex_auth_path().exists());
                assert!(!crate::codex_config::codex_managed_oauth_live_auth_marker_exists());
                ProviderService::switch(state, AppType::Codex, "legacy").unwrap();
                assert!(
                    !crate::codex_config::get_codex_auth_path().exists(),
                    "unbound legacy card must not resurrect stored credentials"
                );
                assert!(state
                    .db
                    .unbind_codex_managed_accounts(&["acct-old".to_string()])
                    .unwrap()
                    .is_empty());
            });
        }
    }


    #[test]
    #[serial]
    fn codex_auth_center_removal_waits_for_provider_switch_lock() {
        with_test_home(|state, _| {
            crate::settings::reload_settings().expect("reload settings");
            tauri::async_runtime::block_on(async {
                state
                    .codex_oauth_manager
                    .add_test_account_with_user_identity(
                        "acct-managed",
                        "managed-access",
                        "managed-user",
                    )
                    .await
                    .expect("seed managed account");
            });

            let switch_guard = tauri::async_runtime::block_on(
                state
                    .proxy_service
                    .lock_switch_for_app(AppType::Codex.as_str()),
            );
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    started_tx.send(()).expect("signal removal start");
                    let result = tauri::async_runtime::block_on(
                        crate::commands::remove_codex_oauth_account_with_switch_lock(
                            state,
                            "acct-managed",
                        ),
                    );
                    done_tx.send(result).expect("send removal result");
                });
                started_rx.recv().expect("wait for removal task");
                assert!(
                    done_rx
                        .recv_timeout(std::time::Duration::from_millis(100))
                        .is_err(),
                    "Auth Center removal must wait while a provider transaction owns the Codex lock"
                );
                drop(switch_guard);
                done_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .expect("removal should finish after lock release")
                    .expect("remove managed account");
            });
            assert!(
                tauri::async_runtime::block_on(state.codex_oauth_manager.list_accounts())
                    .is_empty()
            );
        });
    }



    #[test]
    #[serial]
    fn update_persists_non_current_omo_variants_in_database() {
        with_test_home(|state, _| {
            for category in ["omo", "omo-slim"] {
                let provider = opencode_omo_provider(&format!("{category}-provider"), category);
                state
                    .db
                    .save_provider(AppType::OpenCode.as_str(), &provider)
                    .unwrap_or_else(|err| panic!("seed {category} provider: {err}"));

                let mut updated = provider.clone();
                updated.name = format!("Updated {category}");
                updated.settings_config["agents"]["writer"]["model"] =
                    Value::String(format!("{category}-next-model"));

                ProviderService::update(state, AppType::OpenCode, None, updated)
                    .unwrap_or_else(|err| panic!("update {category} provider: {err}"));

                let saved = state
                    .db
                    .get_provider_by_id(&provider.id, AppType::OpenCode.as_str())
                    .unwrap_or_else(|err| panic!("query updated {category} provider: {err}"))
                    .unwrap_or_else(|| panic!("{category} provider should exist"));

                assert_eq!(saved.name, format!("Updated {category}"));
                assert_eq!(
                    saved.settings_config["agents"]["writer"]["model"],
                    Value::String(format!("{category}-next-model")),
                    "{category} updates should persist in the database"
                );
            }
        });
    }




    #[test]
    #[serial]
    fn universal_provider_icon_is_validated_on_save_and_sync() {
        with_test_home(|state, _home| {
            let mut universal = UniversalProvider::new(
                "icon".into(),
                "Icon".into(),
                "custom".into(),
                "https://api.example".into(),
                "key".into(),
            );
            universal.apps.claude = true;
            // 只有文件头、没有像素数据的「PNG」
            universal.icon =
                Some("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAGAAAABgCAYAAAA=".into());
            assert!(ProviderService::upsert_universal(state, universal.clone()).is_err());
            assert!(state.db.get_universal_provider("icon").unwrap().is_none());

            // 绕过保存直接写进库的坏图标，同步到应用时也不会复制给子供应商
            state.db.save_universal_provider(&universal).unwrap();
            assert!(ProviderService::sync_universal_to_apps(state, "icon").is_err());
            assert!(state
                .db
                .get_provider_by_id("universal-claude-icon", "claude")
                .unwrap()
                .is_none());
        });
    }

    #[test]
    #[serial]
    fn sync_universal_to_apps_preserves_child_metadata() {
        with_test_home(|state, _home| {
            let mut universal = UniversalProvider::new(
                "metadata".into(),
                "Original".into(),
                "custom".into(),
                "https://old.example".into(),
                "old-key".into(),
            );
            universal.apps.claude = true;
            universal.apps.codex = true;
            universal.apps.gemini = true;
            universal.meta = Some(
                serde_json::from_value(json!({
                    "usage_script": {"enabled": false, "language": "javascript", "code": "parent"}
                }))
                .unwrap(),
            );
            state.db.save_universal_provider(&universal).unwrap();
            ProviderService::sync_universal_to_apps(state, &universal.id).unwrap();

            let mut expected = Vec::new();
            for (index, app) in ["claude", "codex", "gemini"].iter().enumerate() {
                let id = format!("universal-{app}-metadata");
                let mut child = state.db.get_provider_by_id(&id, app).unwrap().unwrap();
                assert_eq!(
                    serde_json::to_value(&child.meta).unwrap(),
                    serde_json::to_value(&universal.meta).unwrap()
                );
                child.meta = Some(
                    serde_json::from_value(json!({
                        "usage_script": {"enabled": true, "language": "javascript", "code": app,
                            "apiKey": "usage-only-key", "autoQueryInterval": 15},
                        "commonConfigEnabled": false,
                        "endpointAutoSelect": true
                    }))
                    .unwrap(),
                );
                child.created_at = Some(123 + index as i64);
                child.sort_index = Some(10 + index);
                child.settings_config["local_setting"] = json!(app);
                state.db.save_provider(app, &child).unwrap();
                state
                    .db
                    .add_custom_endpoint(app, &id, "https://extra.example")
                    .unwrap();
                expected.push(child);
            }

            universal.name = "Updated".into();
            universal.base_url = "https://new.example".into();
            universal.api_key = "new-key".into();
            universal.notes = Some("shared note".into());
            universal.models = serde_json::from_value(json!({
                "claude": {"model": "claude-new"},
                "codex": {"model": "codex-new", "reasoningEffort": "low"},
                "gemini": {"model": "gemini-new"}
            }))
            .unwrap();
            // Both absent and present parent metadata must not overwrite child settings.
            for parent_meta in [None, universal.meta.clone()] {
                universal.meta = parent_meta;
                state.db.save_universal_provider(&universal).unwrap();
                ProviderService::sync_universal_to_apps(state, &universal.id).unwrap();
                for (app, before) in ["claude", "codex", "gemini"].iter().zip(&expected) {
                    let after = state
                        .db
                        .get_provider_by_id(&before.id, app)
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        serde_json::to_value(&after.meta).unwrap(),
                        serde_json::to_value(&before.meta).unwrap(),
                        "{app}"
                    );
                    assert_eq!(after.created_at, before.created_at, "{app}");
                    assert_eq!(after.sort_index, before.sort_index, "{app}");
                    assert_eq!(after.name, "Updated");
                    assert_eq!(after.notes.as_deref(), Some("shared note"));
                    assert_eq!(after.settings_config["local_setting"], json!(app));
                    let generated = match *app {
                        "claude" => universal.to_claude_provider(),
                        "codex" => universal.to_codex_provider(),
                        _ => universal.to_gemini_provider(),
                    }
                    .unwrap();
                    let mut expected_settings = before.settings_config.clone();
                    ProviderService::merge_json(&mut expected_settings, &generated.settings_config);
                    assert_eq!(after.settings_config, expected_settings);
                    assert_eq!(
                        state.db.get_all_providers(app).unwrap()[&before.id]
                            .meta
                            .as_ref()
                            .unwrap()
                            .custom_endpoints
                            .len(),
                        1
                    );
                }
            }
        });
    }





}

impl ProviderService {
    pub(crate) fn managed_codex_oauth_account_id(provider: &Provider) -> Option<String> {
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.managed_account_id_for("codex_oauth"))
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
    }

    fn normalize_provider_if_claude(app_type: &AppType, provider: &mut Provider) {
        if matches!(app_type, AppType::Claude) {
            let mut v = provider.settings_config.clone();
            if normalize_claude_models_in_value(&mut v) {
                provider.settings_config = v;
            }
        }
    }


    fn set_provider_live_config_managed(provider: &mut Provider, managed: bool) {
        provider
            .meta
            .get_or_insert_with(Default::default)
            .live_config_managed = Some(managed);
    }

    fn normalize_usage_script_credential_overrides(app_type: &AppType, provider: &mut Provider) {
        let current_credentials = provider.resolve_usage_credentials(app_type);

        let Some(usage_script) = provider
            .meta
            .as_mut()
            .and_then(|meta| meta.usage_script.as_mut())
        else {
            return;
        };

        if usage_script.template_type.as_deref() == Some("token_plan") {
            return;
        }

        if usage_script.api_key.as_deref().is_some_and(|api_key| {
            Self::should_clear_usage_api_key_override(api_key, &current_credentials)
        }) {
            usage_script.api_key = None;
        }

        if usage_script.base_url.as_deref().is_some_and(|base_url| {
            Self::should_clear_usage_base_url_override(base_url, &current_credentials)
        }) {
            usage_script.base_url = None;
        }
    }

    fn should_clear_usage_api_key_override(
        script_api_key: &str,
        current_credentials: &(String, String),
    ) -> bool {
        let candidate = script_api_key.trim();
        if candidate.is_empty() {
            return true;
        }

        let matches_provider_key = |api_key: &str| {
            let api_key = api_key.trim();
            !api_key.is_empty() && api_key == candidate
        };

        matches_provider_key(&current_credentials.1)
    }

    fn should_clear_usage_base_url_override(
        script_base_url: &str,
        current_credentials: &(String, String),
    ) -> bool {
        let candidate = Self::normalize_usage_base_url_for_compare(script_base_url);
        if candidate.is_empty() {
            return true;
        }

        let matches_provider_base_url = |base_url: &str| {
            let base_url = Self::normalize_usage_base_url_for_compare(base_url);
            !base_url.is_empty() && base_url == candidate
        };

        matches_provider_base_url(&current_credentials.0)
    }

    fn normalize_usage_base_url_for_compare(base_url: &str) -> String {
        base_url.trim().trim_end_matches('/').to_string()
    }

    /// List all providers for an app type
    pub fn list(
        state: &AppState,
        app_type: AppType,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let _native_guard = lock_native_provider_operations(state, &app_type);
        if app_type == AppType::Pi {
            return pi::list(state);
        }
        if app_type == AppType::Omp {
            return omp::list(state);
        }
        if matches!(app_type, AppType::Mcode | AppType::Kimi | AppType::Dsh) {
            let native = if matches!(app_type, AppType::Dsh) {
                crate::dsh_config::get_providers()?
            } else if app_type == AppType::Kimi {
                crate::kimi_config::get_providers()?
            } else {
                crate::mcode_config::get_providers()?
            };
            let mut saved = state.db.get_all_providers(app_type.as_str())?;
            let nicknames = if app_type == AppType::Kimi {
                crate::services::kimi_oauth::account_nicknames()
            } else {
                Default::default()
            };
            for (id, config) in &native {
                if !saved.contains_key(id) {
                    let name = config.get("name").and_then(Value::as_str).unwrap_or(id);
                    saved.insert(
                        id.clone(),
                        Provider::with_id(id.clone(), name.into(), config.clone(), None),
                    );
                }
            }
            for (id, provider) in &mut saved {
                if let Some(config) = native.get(id) {
                    if let Some(name) = config.get("name").and_then(Value::as_str) {
                        provider.name = name.into();
                    }
                    provider.settings_config = config.clone();
                }
                Self::set_provider_live_config_managed(provider, native.contains_key(id));
                if app_type == AppType::Kimi {
                    crate::kimi_config::normalize_provider_identity(
                        provider,
                        nicknames.get(id).map(String::as_str),
                    );
                }
                state.db.save_provider(app_type.as_str(), provider)?;
            }
            return Ok(saved);
        }
        state.db.get_all_providers(app_type.as_str())
    }

    /// Get current provider ID
    ///
    /// 使用有效的当前供应商 ID（验证过存在性）。
    /// 优先从本地 settings 读取，验证后 fallback 到数据库的 is_current 字段。
    /// 这确保了云同步场景下多设备可以独立选择供应商，且返回的 ID 一定有效。
    ///
    /// 对于累加模式应用（OpenCode, OpenClaw），不存在"当前供应商"概念，直接返回空字符串。
    pub fn current(state: &AppState, app_type: AppType) -> Result<String, AppError> {
        if app_type == AppType::Kimi {
            return crate::kimi_config::current_provider();
        }
        // Additive mode apps have no "current" provider concept
        if app_type.is_additive_mode() {
            return Ok(String::new());
        }
        // 只有一份激活指针：界面上的「当前」、代理实际出站的那家都是它。
        Self::active_provider(&state.db, &app_type).map(|opt| {
            opt.map(|provider| provider.id).unwrap_or_default()
        })
    }

    /// 当前激活的那一行。切换只改这一份指针，代理与界面都从这里取。
    pub fn active_provider(
        db: &crate::database::Database,
        app_type: &AppType,
    ) -> Result<Option<Provider>, AppError> {
        match crate::settings::get_effective_current_provider(db, app_type)? {
            Some(id) => db.get_provider_by_id(&id, app_type.as_str()),
            None => Ok(None),
        }
    }

    /// 新增供应商之前的规范化和校验，`add` 和 `add_without_switching` 共用。
    fn prepare_new_provider(app_type: &AppType, provider: &mut Provider) -> Result<(), AppError> {
        // Normalize Claude model keys
        Self::normalize_provider_if_claude(app_type, provider);
        Self::validate_provider_settings(app_type, provider)?;
        Self::normalize_usage_script_credential_overrides(app_type, provider);
        if matches!(app_type, AppType::Claude | AppType::Codex | AppType::Gemini) {
            keep_common_config_for_old_versions(provider);
        }
        Ok(())
    }

    /// 只把供应商存进列表，不写客户端配置、不动当前指针（切换式应用）。
    ///
    /// `add` 在还没有当前供应商时会让新的这家直接生效；深链接导入时用户关掉了「导入后立即
    /// 切换」，就不能这样，交给用户之后自己切换。
    pub fn add_without_switching(
        state: &AppState,
        app_type: AppType,
        provider: Provider,
    ) -> Result<bool, AppError> {
        if app_type.is_additive_mode() {
            return Self::add(state, app_type, provider);
        }
        let mut provider = provider;
        Self::prepare_new_provider(&app_type, &mut provider)?;
        state.db.save_provider(app_type.as_str(), &provider)?;
        Ok(true)
    }

    /// Add a new provider
    pub fn add(
        state: &AppState,
        app_type: AppType,
        provider: Provider,
    ) -> Result<bool, AppError> {
        let _native_guard = lock_native_provider_operations(state, &app_type);
        if app_type == AppType::Pi {
            return pi::add(state, provider);
        }
        if app_type == AppType::Omp {
            return omp::add(state, provider);
        }

        let mut provider = provider;
        Self::prepare_new_provider(&app_type, &mut provider)?;
        state.db.save_provider(app_type.as_str(), &provider)?;
        // 还没有激活项时，新加的这一家就是它；否则只进列表，等用户自己切过去。
        // 累加式应用没有「当前」概念，Antigravity 的卡也只用来打开终端。
        if !app_type.is_additive_mode()
            && app_type != AppType::Agy
            && Self::active_provider(&state.db, &app_type)?.is_none()
        {
            crate::settings::set_current_provider(&app_type, Some(&provider.id))?;
            state.db
                .set_current_provider(app_type.as_str(), &provider.id)?;
        }
        Ok(true)
    }

    /// 供应商编辑器里显示的内容：这张卡自己的配置。caswitch 不再推算「切过去之后客户端
    /// 长什么样」，预览就是保存的那份 `settings_config`。
    pub fn editor_view(
        state: &AppState,
        app_type: AppType,
        settings_config: &Value,
        category: Option<&str>,
    ) -> Result<EditorView, AppError> {
        Self::editor_view_with_meta(state, app_type, settings_config, category, None, None)
    }

    /// 带草稿元数据的预览。参数保留给前端现有调用，内容与 [`Self::editor_view`] 相同。
    pub fn editor_view_with_meta(
        _state: &AppState,
        _app_type: AppType,
        settings_config: &Value,
        _category: Option<&str>,
        _provider_id: Option<&str>,
        _meta: Option<&ProviderMeta>,
    ) -> Result<EditorView, AppError> {
        Ok(editor::view(settings_config))
    }

    /// 编辑已有供应商时的 `category`：库里那一行存的是什么就返回什么。
    pub fn editor_category(
        _state: &AppState,
        _app_type: &AppType,
        _provider_id: Option<&str>,
        category: Option<String>,
    ) -> Result<Option<String>, AppError> {
        Ok(category)
    }

    /// 从编辑器新增供应商：和 [`Self::add`] 一样，只存行。
    pub fn add_from_editor(
        state: &AppState,
        app_type: AppType,
        provider: Provider,
        _editor: Option<EditorSave>,
    ) -> Result<bool, AppError> {
        Self::add(state, app_type, provider)
    }

    /// 从编辑器保存供应商：和 [`Self::update`] 一样，只存行。
    pub fn update_from_editor(
        state: &AppState,
        app_type: AppType,
        original_id: Option<&str>,
        provider: Provider,
        _editor: Option<EditorSave>,
    ) -> Result<bool, AppError> {
        Self::update(state, app_type, original_id, provider)
    }

    /// Extract common config snippet from a config value (e.g. editor content).
    pub fn extract_common_config_snippet_from_settings(
        app_type: AppType,
        settings_config: &Value,
    ) -> Result<String, AppError> {
        match app_type {
            AppType::Claude => Self::extract_claude_common_config(settings_config),
            AppType::ClaudeDesktop => Ok(String::new()),
            AppType::Codex => Self::extract_codex_common_config(settings_config),
            AppType::Gemini => Self::extract_gemini_common_config(settings_config),
            AppType::GrokBuild => Ok(String::new()),
            AppType::OpenCode => Self::extract_opencode_common_config(settings_config),
            AppType::OpenClaw => Self::extract_openclaw_common_config(settings_config),
            AppType::Hermes => Ok(String::new()), // Hermes doesn't use common config snippets
            AppType::Pi
            | AppType::Mcode
            | AppType::Kimi
            | AppType::Dsh
            | AppType::Omp
            | AppType::Agy => Ok(String::new()),
        }
    }

    /// 判断一个 env / 顶层配置键名是否为凭据/机密：凡命中一律不得写入共享的
    /// 通用配置片段。**故意从严**——多剥一个非机密键只是它不被共享（可恢复的小
    /// 不便），漏剥一个凭据则会把密钥注入到每个供应商（不可恢复的泄漏）。因此用
    /// 模式匹配覆盖整类，而非枚举具体名字（枚举永远会漏掉下一个 `*_API_KEY`）。
    ///
    /// 覆盖：Anthropic / OpenRouter / Google / OpenAI / Gemini 等 `*_API_KEY`
    /// （Claude provider 的凭据见 `Provider::resolve_usage_credentials`，确实支持
    /// `OPENROUTER_API_KEY` / `GOOGLE_API_KEY` 等回退）、各类 `*_AUTH_TOKEN` /
    /// 单数 `*_TOKEN`、AWS Bedrock / Vertex 凭据、通用 secret / password /
    /// 私钥命名，以及发往上游的自定义请求头、Cookie、Authorization。
    pub(crate) fn is_sensitive_config_key(name: &str) -> bool {
        let upper = name.to_ascii_uppercase();

        // 单数 `_TOKEN` 命中 AWS_SESSION_TOKEN 等，但**不**误伤复数 `_TOKENS`
        // （CLAUDE_CODE_MAX_OUTPUT_TOKENS / MAX_THINKING_TOKENS 是正常可共享配置）。
        const SENSITIVE_SUFFIXES: &[&str] = &[
            // 裸 `_KEY` 是最常见的凭据写法（OPENAI_KEY / GROQ_KEY / XAI_KEY…），
            // 必须单列：只枚举 `_API_KEY` / `_ACCESS_KEY` 这些子类，等于把最普通
            // 的那一种漏在外面。下面几条 `_*_KEY` 被它蕴含，保留是为了说明覆盖面。
            "_KEY",
            "_API_KEY",
            "_ACCESS_KEY",
            "_ACCESS_KEY_ID",
            "_KEY_ID",
            "_PRIVATE_KEY",
            // 不带分隔符的复合写法各走各的后缀：`_KEY` 够不着 `..._APIKEY`
            // （倒数第四个字符是 I 不是下划线）。VOLC_ACCESSKEY 是火山引擎文档
            // 里的正式变量名，本仓库就实现了火山 AK/SK 用量查询。
            "_APIKEY",
            "_ACCESSKEY",
            "_SECRETKEY",
            "_APITOKEN",
            "_AUTH_TOKEN",
            "_TOKEN",
            // GITHUB_PAT / GITLAB_PAT 等 personal access token 的惯用写法，
            // 既不含 TOKEN 也不含 KEY，前面每一条规则都够不着。
            "_PAT",
            // 口令类的常见缩写。`_PASS` 不会误伤 `*_BYPASS`（那个以 `_BYPASS`
            // 结尾），`_PWD` 也不会误伤 shell 的 PWD / OLDPWD。
            "_PWD",
            "_PASS",
            "_PASSPHRASE",
            "_CREDS",
            // 发往上游的自定义请求头（ANTHROPIC_CUSTOM_HEADERS、
            // GEMINI_CLI_CUSTOM_HEADERS）：常见写法是 `Authorization: Bearer …`
            // 或 `Cookie: …`，整串就是凭据。只认 `_CUSTOM_HEADERS`，不按 HEADER
            // 一刀切：CLAUDE_CODE_ATTRIBUTION_HEADER 是普通开关，
            // OTEL_EXPORTER_OTLP_HEADERS 发往用户自己的遥测端点，都应照常共享。
            "_CUSTOM_HEADERS",
        ];
        const SENSITIVE_EXACT: &[&str] = &[
            "APIKEY",
            "API_KEY",
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "CREDENTIALS",
            "HEADERS",
        ];
        // contains：覆盖 AWS_SECRET_ACCESS_KEY / *_CLIENT_SECRET /
        // GOOGLE_APPLICATION_CREDENTIALS / AWS_BEARER_TOKEN_BEDROCK 等变体。
        const SENSITIVE_CONTAINS: &[&str] = &[
            "SECRET",
            "PASSWORD",
            "PASSWD",
            "CREDENTIAL",
            "PRIVATE_KEY",
            "BEARER_TOKEN",
            "COOKIE",
            "AUTHORIZATION",
        ];

        SENSITIVE_EXACT.contains(&upper.as_str())
            || SENSITIVE_SUFFIXES.iter().any(|s| upper.ends_with(s))
            || SENSITIVE_CONTAINS.iter().any(|c| upper.contains(c))
    }

    /// Claude `env` 里的键能否进通用配置片段。
    ///
    /// 片段会合并进每一家勾选了它的供应商，所以只能放与供应商无关的设置。关键字段
    /// （请求发到哪、凭什么鉴权、哪个模型名、哪种协议）一旦进了片段，就会跟着切换带给
    /// 下一家：从 Bedrock 切到官方后，官方的 live 里还留着 `CLAUDE_CODE_USE_BEDROCK=1`，
    /// Claude Code 继续走 Bedrock。凭据另由 `is_sensitive_config_key` 统一剥离。
    fn claude_env_key_is_shared(key: &str) -> bool {
        // Context limits follow the actual upstream model. Sharing these
        // across providers can cap GPT/Kimi to the wrong window and make
        // Claude Code compact too early or miss the upstream limit.
        const UPSTREAM_WINDOW_KEYS: &[&str] = &[
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
        ];

        !crate::floor::claude_floor_env(key)
            && !UPSTREAM_WINDOW_KEYS.contains(&key)
            && !Self::is_sensitive_config_key(key)
    }

    /// Claude 顶层键能否进通用配置片段，口径同 [`Self::claude_env_key_is_shared`]。
    /// `model` 是 `/model` 保存的选择，属于当时那一家。
    fn claude_top_key_is_shared(key: &str) -> bool {
        !crate::floor::claude_floor_top(key) && !Self::is_sensitive_config_key(key)
    }

    /// Extract common config for Claude (JSON format)
    fn extract_claude_common_config(settings: &Value) -> Result<String, AppError> {
        let mut config = settings.clone();

        if let Some(obj) = config.as_object_mut() {
            if let Some(Value::Object(env)) = obj.get_mut("env") {
                env.retain(|key, _| Self::claude_env_key_is_shared(key));
            }
            obj.retain(|key, value| match key.as_str() {
                "env" => !value.as_object().is_some_and(|env| env.is_empty()),
                _ => Self::claude_top_key_is_shared(key),
            });
        }

        // Check if result is empty
        if config.as_object().is_none_or(|obj| obj.is_empty()) {
            return Ok("{}".to_string());
        }

        serde_json::to_string_pretty(&config)
            .map_err(|e| AppError::Message(format!("Serialization failed: {e}")))
    }

    /// Extract common config for Codex (TOML format)
    fn extract_codex_common_config(settings: &Value) -> Result<String, AppError> {
        // Codex config is stored as { "auth": {...}, "config": "toml string" }
        let config_toml = settings
            .get("config")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if config_toml.is_empty() {
            return Ok(String::new());
        }

        let mut doc = config_toml
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| AppError::Message(format!("TOML parse error: {e}")))?;

        // 关键字段（选路、模型名、推理档位等，见 `live::floor`）归供应商，不进片段：
        // 片段已冻结，收进去的值会从行里被迁移剥掉、再也写不回 live。
        let root = doc.as_table_mut();
        for key in crate::floor::CODEX_FLOOR_TOP {
            root.remove(key);
        }
        for path in crate::floor::CODEX_FLOOR_NESTED {
            let [parent, key] = path else { continue };
            if let Some(table) = root
                .get_mut(parent)
                .and_then(|item| item.as_table_like_mut())
            {
                table.remove(key);
                if table.is_empty() {
                    root.remove(parent);
                }
            }
        }

        // Remove entire model_providers table (provider-specific configuration)
        root.remove("model_providers");

        // MCP 服务器归 DB mcp_servers 表所有：进了共享片段会绕过按应用的
        // 启用状态被合并进所有勾选通用配置的供应商，且在通用配置编辑框里
        // 显示为一份"重复"的 MCP 配置。
        root.remove("mcp_servers");
        // 历史错误格式 [mcp.servers] 一并剥离（与 strip_codex_mcp_servers_from_settings
        // 一致）：sync_all_enabled 只管理 [mcp_servers.*]，legacy 形态一旦进了
        // 片段就会被合并进所有供应商，且没有任何同步路径能清掉这个孤儿。
        if let Some(mcp_tbl) = root
            .get_mut("mcp")
            .and_then(|item| item.as_table_like_mut())
        {
            mcp_tbl.remove("servers");
            if mcp_tbl.is_empty() {
                root.remove("mcp");
            }
        }

        // cc-switch 写 live 时注入的产物一律不进共享片段：
        // - experimental_bearer_token 正常写在 [model_providers.<id>] 内（上面
        //   整表已剥），但无活跃路由 / 内建保留 id / 路由表缺失三种 fallback
        //   会落在顶层——不剥等于把 API 密钥写进共享片段。
        root.remove("experimental_bearer_token");
        // - model_catalog_json 指向按供应商生成的 catalog 投影文件（DB 为 SSOT）。
        root.remove("model_catalog_json");
        // - web_search 只剥 cc-switch 注入的 "disabled" 哨兵；用户手设的其它值
        //   属于可共享偏好，保留。
        if root
            .get(crate::codex_config::CODEX_WEB_SEARCH_FIELD)
            .and_then(|item| item.as_str())
            == Some(crate::codex_config::CODEX_WEB_SEARCH_DISABLED)
        {
            root.remove(crate::codex_config::CODEX_WEB_SEARCH_FIELD);
        }

        // Clean up multiple empty lines (keep at most one blank line).
        let mut cleaned = String::new();
        let mut blank_run = 0usize;
        for line in doc.to_string().lines() {
            if line.trim().is_empty() {
                blank_run += 1;
                if blank_run <= 1 {
                    cleaned.push('\n');
                }
                continue;
            }
            blank_run = 0;
            cleaned.push_str(line);
            cleaned.push('\n');
        }

        Ok(cleaned.trim().to_string())
    }

    /// Extract common config for Gemini (JSON format)
    ///
    /// Extracts `.env` values while excluding provider-specific credentials:
    /// - GOOGLE_GEMINI_BASE_URL
    /// - GEMINI_API_KEY
    fn extract_gemini_common_config(settings: &Value) -> Result<String, AppError> {
        let env = settings.get("env").and_then(|v| v.as_object());

        let mut snippet = serde_json::Map::new();
        if let Some(env) = env {
            for (key, value) in env {
                // 关键字段（地址、凭据、模型名、Vertex 选择器）归供应商，不进片段：
                // 片段已冻结，新版不再合并它，收进去的值会从行里被迁移剥掉、再也写不回
                // live（首启导入的 `GEMINI_MODEL` 就是这样丢的）。凭据另交给
                // `is_sensitive_config_key` 统一模式匹配（与 Claude 提取器一致），
                // 兜住关键字段清单以外的 `*_API_KEY`、`*_TOKEN`。
                if crate::floor::gemini_floor_env(key) || Self::is_sensitive_config_key(key) {
                    continue;
                }
                let Value::String(v) = value else {
                    continue;
                };
                let trimmed = v.trim();
                if !trimmed.is_empty() {
                    snippet.insert(key.to_string(), Value::String(trimmed.to_string()));
                }
            }
        }

        if snippet.is_empty() {
            return Ok("{}".to_string());
        }

        serde_json::to_string_pretty(&Value::Object(snippet))
            .map_err(|e| AppError::Message(format!("Serialization failed: {e}")))
    }

    /// Extract common config for OpenCode (JSON format)
    fn extract_opencode_common_config(settings: &Value) -> Result<String, AppError> {
        // OpenCode uses a different config structure with npm, options, models
        // For common config, we exclude provider-specific fields like apiKey
        let mut config = settings.clone();

        // Remove provider-specific fields
        if let Some(obj) = config.as_object_mut() {
            for key in ["options", "settings"] {
                if let Some(options) = obj.get_mut(key).and_then(|v| v.as_object_mut()) {
                    options.remove("apiKey");
                    options.remove("baseURL");
                }
            }
            // Keep npm and models as they might be common
        }

        if config.is_null() || (config.is_object() && config.as_object().unwrap().is_empty()) {
            return Ok("{}".to_string());
        }

        serde_json::to_string_pretty(&config)
            .map_err(|e| AppError::Message(format!("Serialization failed: {e}")))
    }

    /// Extract common config for OpenClaw (JSON format)
    fn extract_openclaw_common_config(settings: &Value) -> Result<String, AppError> {
        // OpenClaw uses a different config structure with baseUrl, apiKey, api, models
        // For common config, we exclude provider-specific fields like apiKey
        let mut config = settings.clone();

        // Remove provider-specific fields
        if let Some(obj) = config.as_object_mut() {
            obj.remove("apiKey");
            obj.remove("baseUrl");
            // Keep api and models as they might be common
        }

        if config.is_null() || (config.is_object() && config.as_object().unwrap().is_empty()) {
            return Ok("{}".to_string());
        }

        serde_json::to_string_pretty(&config)
            .map_err(|e| AppError::Message(format!("Serialization failed: {e}")))
    }

    /// Update a provider
    pub fn update(
        state: &AppState,
        app_type: AppType,
        original_id: Option<&str>,
        provider: Provider,
    ) -> Result<bool, AppError> {
        let _native_guard = lock_native_provider_operations(state, &app_type);
        if app_type == AppType::Pi {
            return pi::update(state, original_id, provider);
        }
        if app_type == AppType::Omp {
            return omp::update(state, original_id, provider);
        }

        let mut provider = provider;
        let original_id = original_id.unwrap_or(provider.id.as_str()).to_string();
        let provider_id_changed = original_id != provider.id;
        Self::normalize_provider_if_claude(&app_type, &mut provider);
        Self::validate_provider_settings(&app_type, &provider)?;
        Self::normalize_usage_script_credential_overrides(&app_type, &mut provider);

        if !provider_id_changed {
            state.db.save_provider(app_type.as_str(), &provider)?;
            return Ok(true);
        }

        if !app_type.is_additive_mode() {
            return Err(AppError::Message(
                "Only additive-mode providers support changing provider key".to_string(),
            ));
        }
        let Some(existing_provider) = state
            .db
            .get_provider_by_id(&original_id, app_type.as_str())?
        else {
            return Err(AppError::Message(format!(
                "Original provider '{}' does not exist in app '{}'",
                original_id,
                app_type.as_str()
            )));
        };
        // OMO / OMO Slim 的「当前」标记按 provider_id 记在外面的表里，改名会让标记指向
        // 一个不存在的行，后面的同步会静默失效。
        if matches!(app_type, AppType::OpenCode)
            && matches!(
                existing_provider.category.as_deref(),
                Some("omo") | Some("omo-slim")
            )
        {
            return Err(AppError::Message(
                "Provider key cannot be changed for OMO/OMO Slim providers".to_string(),
            ));
        }
        if state
            .db
            .get_provider_by_id(&provider.id, app_type.as_str())?
            .is_some()
        {
            return Err(AppError::Message(format!(
                "Provider '{}' already exists in app '{}'",
                provider.id,
                app_type.as_str()
            )));
        }

        let was_current =
            Self::active_provider(&state.db, &app_type)?.map(|row| row.id) .as_deref()
                == Some(original_id.as_str());
        state.db.save_provider(app_type.as_str(), &provider)?;
        state.db.delete_provider(app_type.as_str(), &original_id)?;
        if was_current {
            crate::settings::set_current_provider(&app_type, Some(&provider.id))?;
            state.db
                .set_current_provider(app_type.as_str(), &provider.id)?;
        }
        Ok(true)
    }

    pub(crate) fn update_pi_usage_script(
        state: &AppState,
        app_type: AppType,
        id: &str,
        script: crate::provider::UsageScript,
    ) -> Result<bool, AppError> {
        pi::update_usage_script(state, app_type, id, script)
    }

    /// Delete a provider
    ///
    /// 直连指针或代理路由指着的供应商不能删（见 `mode::current::is_referenced`）。
    /// 对于累加模式应用（OpenCode, OpenClaw），可以随时删除任意供应商，同时从 live 配置中移除。
    pub fn delete(state: &AppState, app_type: AppType, id: &str) -> Result<(), AppError> {
        let _native_guard = lock_native_provider_operations(state, &app_type);
        if app_type == AppType::Pi {
            return pi::delete(state, id);
        }
        if app_type == AppType::Omp {
            return omp::delete(state, id);
        }
        // 正在用的那家不能删：界面上的「当前」和代理实际出站的是同一行。
        if app_type != AppType::Agy
            && Self::active_provider(&state.db, &app_type)?
                .map(|provider| provider.id)
                .as_deref()
                == Some(id)
        {
            return Err(AppError::Message(
                "无法删除当前正在使用的供应商".to_string(),
            ));
        }
        state.db.delete_provider(app_type.as_str(), id)?;
        Ok(())
    }

    /// The signed-in connection node of a Kimi official card: Kimi's live node
    /// first, else the saved card (an official card removed from Kimi's config).
    fn kimi_official_connection(state: &AppState, id: &str) -> Result<Option<Value>, AppError> {
        if let Some(live) = crate::kimi_config::get_providers()?.get(id) {
            return Ok(
                crate::kimi_config::is_official_provider(live).then(|| live["provider"].clone())
            );
        }
        Ok(state
            .db
            .get_provider_by_id(id, "kimi")?
            .filter(|saved| crate::kimi_config::is_official_provider(&saved.settings_config))
            .map(|saved| saved.settings_config["provider"].clone()))
    }

    /// Deleting an official account that is still in Kimi's config goes through
    /// the Authorization Center (it also deletes the login); remove it first.
    fn ensure_kimi_provider_editable(id: &str) -> Result<(), AppError> {
        let native = crate::kimi_config::get_providers()?;
        if native
            .get(id)
            .is_some_and(crate::kimi_config::is_official_provider)
        {
            return Err(AppError::InvalidInput(
                "Manage Kimi official accounts in the Authorization Center".into(),
            ));
        }
        Ok(())
    }

    /// 切换供应商。
    ///
    /// - 代理模式：只换代理路由，直连指针不变；契约没变时客户端文件不读也不写。
    /// - 直连模式：切换式应用只替换客户端文件里的关键字段（不回填），文件和指针在同一个
    ///   操作里提交；累加式应用按各自的规则写入。
    pub fn switch(state: &AppState, app_type: AppType, id: &str) -> Result<SwitchResult, AppError> {
        let app = app_type.as_str().to_string();
        let result = Self::switch_inner(state, app_type, id);
        match &result {
            Ok(result) if result.warnings.is_empty() => {
                log::info!("[SWITCH] {app} 切到 {id} 完成");
            }
            Ok(result) => {
                log::warn!(
                    "[SWITCH] {app} 切到 {id} 完成，有警告: {:?}",
                    result.warnings
                );
            }
            // 规则拒绝：什么都没改。
            Err(error @ AppError::Localized { key, .. })
                if *key == "switch.official_blocked_by_proxy"
                    || *key == "agy.switch.unsupported" =>
            {
                log::info!("[SWITCH] {app} 拒绝切到 {id}: {error}");
            }
            Err(error) => log::error!(
                "[SWITCH] {app} 切到 {id} 失败: {}",
                crate::error_for_log(&error.to_string())
            ),
        }
        result
    }

    fn switch_inner(
        state: &AppState,
        app_type: AppType,
        id: &str,
    ) -> Result<SwitchResult, AppError> {
        let _native_guard = lock_native_provider_operations(state, &app_type);
        if app_type == AppType::Pi {
            return pi::enable(state, id);
        }
        // Antigravity 的卡只作用于它打开的终端，没有「启用」（见 agy_config 模块说明）。
        if app_type == AppType::Agy {
            return Err(AppError::localized(
                "agy.switch.unsupported",
                "Antigravity 的卡片不需要启用，用「打开终端」启动即可",
                "Antigravity cards are not enabled; use Open terminal to start one",
            ));
        }

        if app_type == AppType::Omp {
            return omp::enable(state, id);
        }

        // Check if provider exists
        let providers = state.db.get_all_providers(app_type.as_str())?;
        let _provider = providers
            .get(id)
            .ok_or_else(|| AppError::Message(format!("供应商 {id} 不存在")))?;

        if matches!(app_type, AppType::Dsh) {
            let mut provider = _provider.clone();
            Self::set_provider_live_config_managed(&mut provider, true);
            crate::dsh_config::save_and_commit(
                id,
                Some(provider.settings_config.clone()),
                false,
                || state.db.save_provider(app_type.as_str(), &provider),
            )?;
            return Ok(SwitchResult::default());
        }
        if app_type == AppType::Kimi {
            crate::services::kimi_oauth::ensure_profile_signed_in(&_provider.settings_config)?;
            let mut provider = _provider.clone();
            Self::set_provider_live_config_managed(&mut provider, true);
            crate::kimi_config::switch_and_commit(
                id,
                provider.settings_config.clone(),
                |config| {
                    provider.settings_config = config;
                    state.db.save_provider("kimi", &provider)
                },
            )?;
            return Ok(SwitchResult::default());
        }

        // OMO / OMO Slim 是 OpenCode 里互斥的两个变体：切到其中一个就清掉另一个的当前标记。
        if matches!(app_type, AppType::OpenCode) {
            if let Some(category @ ("omo" | "omo-slim")) = _provider.category.as_deref() {
                state
                    .db
                    .set_omo_provider_current(app_type.as_str(), id, category)?;
                return Ok(SwitchResult::default());
            }
        }

        // caswitch 的「切换」只做一件事：把这一行设为激活。代理的四条入口都按它出站，
        // 客户端地址由用户自己指向本地代理——caswitch 不再改写任何客户端配置文件。
        if _provider.category.as_deref() == Some("official")
            && !official_provider_supports_proxy_takeover(&app_type, _provider)
        {
            return Err(AppError::localized(
                "switch.official_blocked_by_proxy",
                "官方供应商不能经本地代理访问，使用代理访问官方 API 可能导致账号被封禁。请选择第三方供应商，或直接使用官方客户端。",
                "Official providers cannot be routed through the local proxy: proxying official APIs may lead to account bans. Pick a third-party provider, or use the official client directly.",
            ));
        }
        if !app_type.is_additive_mode() {
            log::info!("[SWITCH] {} 激活 {id}", app_type.as_str());
            crate::settings::set_current_provider(&app_type, Some(id))?;
            state.db.set_current_provider(app_type.as_str(), id)?;
        }
        Ok(SwitchResult::default())
    }


    /// Get custom endpoints list (re-export)
    pub fn get_custom_endpoints(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<Vec<CustomEndpoint>, AppError> {
        endpoints::get_custom_endpoints(state, app_type, provider_id)
    }

    /// Add custom endpoint (re-export)
    pub fn add_custom_endpoint(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        endpoints::add_custom_endpoint(state, app_type, provider_id, url)
    }

    /// Remove custom endpoint (re-export)
    pub fn remove_custom_endpoint(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        endpoints::remove_custom_endpoint(state, app_type, provider_id, url)
    }

    /// Update endpoint last used timestamp (re-export)
    pub fn update_endpoint_last_used(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        url: String,
    ) -> Result<(), AppError> {
        endpoints::update_endpoint_last_used(state, app_type, provider_id, url)
    }

    /// Update provider sort order
    pub fn update_sort_order(
        state: &AppState,
        app_type: AppType,
        updates: Vec<ProviderSortUpdate>,
    ) -> Result<bool, AppError> {
        let updates: Vec<(String, usize)> = updates
            .into_iter()
            .map(|update| (update.id, update.sort_index))
            .collect();
        state
            .db
            .update_provider_sort_indexes(app_type.as_str(), &updates)?;
        Ok(true)
    }

    /// Query provider usage (re-export)
    pub async fn query_usage(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
    ) -> Result<UsageResult, AppError> {
        usage::query_usage(state, app_type, provider_id).await
    }

    /// Test usage script (re-export)
    #[allow(clippy::too_many_arguments)]
    pub async fn test_usage_script(
        state: &AppState,
        app_type: AppType,
        provider_id: &str,
        script_code: &str,
        timeout: u64,
        api_key: Option<&str>,
        base_url: Option<&str>,
        access_token: Option<&str>,
        user_id: Option<&str>,
        template_type: Option<&str>,
    ) -> Result<UsageResult, AppError> {
        usage::test_usage_script(
            state,
            app_type,
            provider_id,
            script_code,
            timeout,
            api_key,
            base_url,
            access_token,
            user_id,
            template_type,
        )
        .await
    }

    fn validate_provider_settings(app_type: &AppType, provider: &Provider) -> Result<(), AppError> {
        icon::validate_icon(provider.icon.as_deref())?;
        match app_type {
            AppType::Claude => {
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.claude.settings.not_object",
                        "Claude 配置必须是 JSON 对象",
                        "Claude configuration must be a JSON object",
                    ));
                }
            }
            AppType::ClaudeDesktop => {
                crate::claude_desktop_config::validate_provider(provider)?;
            }
            AppType::Codex => {
                let settings = provider.settings_config.as_object().ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.settings.not_object",
                        "Codex 配置必须是 JSON 对象",
                        "Codex configuration must be a JSON object",
                    )
                })?;

                let auth = settings.get("auth").ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.auth.missing",
                        format!("供应商 {} 缺少 auth 配置", provider.id),
                        format!("Provider {} is missing auth configuration", provider.id),
                    )
                })?;
                if !auth.is_object() {
                    return Err(AppError::localized(
                        "provider.codex.auth.not_object",
                        format!("供应商 {} 的 auth 配置必须是 JSON 对象", provider.id),
                        format!(
                            "Provider {} auth configuration must be a JSON object",
                            provider.id
                        ),
                    ));
                }

                if let Some(config_value) = settings.get("config") {
                    if !(config_value.is_string() || config_value.is_null()) {
                        return Err(AppError::localized(
                            "provider.codex.config.invalid_type",
                            "Codex config 字段必须是字符串",
                            "Codex config field must be a string",
                        ));
                    }
                    if let Some(cfg_text) = config_value.as_str() {
                        crate::codex_config::validate_config_toml(cfg_text)?;
                    }
                }
            }
            AppType::Gemini => {
                use crate::gemini_config::validate_gemini_settings;
                validate_gemini_settings(&provider.settings_config)?
            }
            AppType::Agy => {
                use crate::gemini_config::validate_gemini_settings;
                validate_gemini_settings(&provider.settings_config)?
            }
            AppType::GrokBuild => {
                let settings = provider.settings_config.as_object().ok_or_else(|| {
                    AppError::localized(
                        "provider.grokbuild.settings.not_object",
                        "Grok Build 配置必须是 JSON 对象",
                        "Grok Build configuration must be a JSON object",
                    )
                })?;
                let config = settings
                    .get("config")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.grokbuild.config.missing",
                            "Grok Build 配置缺少 config 字段",
                            "Grok Build configuration is missing the config field",
                        )
                    })?;
                if provider.category.as_deref() == Some("official") {
                    // 官方条目走 Grok CLI 自带 OAuth：空 config 合法，
                    // 其余内容只要求 TOML 语法合法。
                    crate::grok_config::validate_config_toml_syntax(config)?;
                } else {
                    crate::grok_config::validate_config_toml(config)?;
                }
            }
            AppType::OpenCode => {
                // OpenCode uses a different config structure: { npm, options, models }
                // Basic validation - must be an object
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.opencode.settings.not_object",
                        "OpenCode 配置必须是 JSON 对象",
                        "OpenCode configuration must be a JSON object",
                    ));
                }
                // The row is saved before the live write, which would refuse an invalid
                // native declaration only after the database already holds it.
                if !matches!(provider.category.as_deref(), Some("omo") | Some("omo-slim")) {
                    let (fragment, format) = crate::opencode_config::provider_fragment(
                        &provider.id,
                        &provider.settings_config,
                        provider.opencode_config_format(),
                    )?;
                    if format == crate::provider::OpenCodeConfigFormat::V2 {
                        crate::opencode_config::validate_native_provider(&provider.id, fragment)?;
                    }
                }
            }
            AppType::OpenClaw => {
                // OpenClaw uses config structure: { baseUrl, apiKey, api, models }
                // Basic validation - must be an object
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.openclaw.settings.not_object",
                        "OpenClaw 配置必须是 JSON 对象",
                        "OpenClaw configuration must be a JSON object",
                    ));
                }
            }
            AppType::Hermes => {
                // Hermes: accept any JSON object for now
                if !provider.settings_config.is_object() {
                    return Err(AppError::localized(
                        "provider.hermes.settings.not_object",
                        "Hermes 配置必须是 JSON 对象",
                        "Hermes configuration must be a JSON object",
                    ));
                }
            }
            AppType::Dsh => {
                crate::dsh_config::validate_provider(&provider.id, &provider.settings_config)?;
            }
            AppType::Kimi => {
                crate::kimi_config::validate_provider(&provider.id, &provider.settings_config)?
            }
            AppType::Mcode => {
                crate::mcode_config::validate_provider(&provider.id, &provider.settings_config)?
            }
            AppType::Pi => {
                crate::pi_config::validate_provider_node(&provider.id, &provider.settings_config)?;
            }
            AppType::Omp => {
                crate::omp_config::validate_provider(&provider.id, &provider.settings_config)?;
            }
        }

        // Validate and clean UsageScript configuration (common for all app types)
        if let Some(meta) = &provider.meta {
            if let Some(usage_script) = &meta.usage_script {
                validate_usage_script(usage_script)?;
            }
        }

        Ok(())
    }

    #[allow(dead_code)]
    fn extract_credentials(
        provider: &Provider,
        app_type: &AppType,
    ) -> Result<(String, String), AppError> {
        match app_type {
            AppType::Claude => {
                let env = provider
                    .settings_config
                    .get("env")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.claude.env.missing",
                            "配置格式错误: 缺少 env",
                            "Invalid configuration: missing env section",
                        )
                    })?;

                let api_key = env
                    .get("ANTHROPIC_AUTH_TOKEN")
                    .or_else(|| env.get("ANTHROPIC_API_KEY"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.claude.api_key.missing",
                            "缺少 API Key",
                            "API key is missing",
                        )
                    })?
                    .to_string();

                let base_url = env
                    .get("ANTHROPIC_BASE_URL")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.claude.base_url.missing",
                            "缺少 ANTHROPIC_BASE_URL 配置",
                            "Missing ANTHROPIC_BASE_URL configuration",
                        )
                    })?
                    .to_string();

                Ok((api_key, base_url))
            }
            AppType::GrokBuild => {
                let config_toml = provider
                    .settings_config
                    .get("config")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.grokbuild.config.missing",
                            "Grok Build 配置缺少 config 字段",
                            "Grok Build configuration is missing the config field",
                        )
                    })?;
                let (base_url, api_key) = crate::grok_config::extract_credentials(config_toml)
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.grokbuild.credentials.missing",
                            "Grok Build 配置缺少 Base URL 或 API Key",
                            "Grok Build configuration is missing the base URL or API key",
                        )
                    })?;
                Ok((api_key, base_url))
            }
            AppType::ClaudeDesktop => {
                let credentials =
                    crate::claude_desktop_config::direct_gateway_credentials(provider)?;
                Ok((credentials.api_key, credentials.base_url))
            }
            AppType::Codex => {
                let _auth = provider
                    .settings_config
                    .get("auth")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.codex.auth.missing",
                            "配置格式错误: 缺少 auth",
                            "Invalid configuration: missing auth section",
                        )
                    })?;

                let config_toml = provider
                    .settings_config
                    .get("config")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let api_key = crate::codex_config::extract_codex_api_key(
                    provider.settings_config.get("auth"),
                    Some(config_toml),
                )
                .ok_or_else(|| {
                    AppError::localized(
                        "provider.codex.api_key.missing",
                        "缺少 API Key",
                        "API key is missing",
                    )
                })?;

                let base_url = if config_toml.contains("base_url") {
                    let re = Regex::new(r#"base_url\s*=\s*["']([^"']+)["']"#).map_err(|e| {
                        AppError::localized(
                            "provider.regex_init_failed",
                            format!("正则初始化失败: {e}"),
                            format!("Failed to initialize regex: {e}"),
                        )
                    })?;
                    re.captures(config_toml)
                        .and_then(|caps| caps.get(1))
                        .map(|m| m.as_str().to_string())
                        .ok_or_else(|| {
                            AppError::localized(
                                "provider.codex.base_url.invalid",
                                "config.toml 中 base_url 格式错误",
                                "base_url in config.toml has invalid format",
                            )
                        })?
                } else {
                    return Err(AppError::localized(
                        "provider.codex.base_url.missing",
                        "config.toml 中缺少 base_url 配置",
                        "base_url is missing from config.toml",
                    ));
                };

                Ok((api_key, base_url))
            }
            AppType::Gemini | AppType::Agy => {
                use crate::gemini_config::json_to_env;

                let env_map = json_to_env(&provider.settings_config)?;

                let api_key = env_map.get("GEMINI_API_KEY").cloned().ok_or_else(|| {
                    AppError::localized(
                        "gemini.missing_api_key",
                        "缺少 GEMINI_API_KEY",
                        "Missing GEMINI_API_KEY",
                    )
                })?;

                let base_url = env_map
                    .get("GOOGLE_GEMINI_BASE_URL")
                    .cloned()
                    .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

                Ok((api_key, base_url))
            }
            AppType::OpenCode => {
                // OpenCode uses options.apiKey and options.baseURL
                let options = provider
                    .settings_config
                    .get("options")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.opencode.options.missing",
                            "配置格式错误: 缺少 options",
                            "Invalid configuration: missing options section",
                        )
                    })?;

                let api_key = options
                    .get("apiKey")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.opencode.api_key.missing",
                            "缺少 API Key",
                            "API key is missing",
                        )
                    })?
                    .to_string();

                let base_url = options
                    .get("baseURL")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                Ok((api_key, base_url))
            }
            AppType::Dsh => Ok((
                provider
                    .settings_config
                    .pointer("/options/apiKey")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                provider
                    .settings_config
                    .pointer("/options/baseURL")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
            )),
            AppType::Kimi => Ok((
                provider
                    .settings_config
                    .pointer("/provider/api_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                provider
                    .settings_config
                    .pointer("/provider/base_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
            )),
            AppType::OpenClaw | AppType::Hermes | AppType::Pi | AppType::Mcode | AppType::Omp => {
                // These native formats use apiKey and baseUrl directly on the object.
                let api_key = provider
                    .settings_config
                    .get("apiKey")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        AppError::localized(
                            "provider.openclaw.api_key.missing",
                            "缺少 API Key",
                            "API key is missing",
                        )
                    })?
                    .to_string();

                let base_url = provider
                    .settings_config
                    .get("baseUrl")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                Ok((api_key, base_url))
            }
        }
    }
}

/// Normalize Claude model keys in a JSON value
///
/// Reads old key (ANTHROPIC_SMALL_FAST_MODEL), writes new keys (DEFAULT_*), and deletes old key.
pub(crate) fn normalize_claude_models_in_value(settings: &mut Value) -> bool {
    let mut changed = false;
    let env = match settings.get_mut("env").and_then(|v| v.as_object_mut()) {
        Some(obj) => obj,
        None => return changed,
    };

    let model = env
        .get("ANTHROPIC_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let small_fast = env
        .get("ANTHROPIC_SMALL_FAST_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let current_haiku = env
        .get("ANTHROPIC_DEFAULT_HAIKU_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let current_sonnet = env
        .get("ANTHROPIC_DEFAULT_SONNET_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let current_opus = env
        .get("ANTHROPIC_DEFAULT_OPUS_MODEL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let target_haiku = current_haiku
        .or_else(|| small_fast.clone())
        .or_else(|| model.clone());
    let target_sonnet = current_sonnet
        .or_else(|| model.clone())
        .or_else(|| small_fast.clone());
    let target_opus = current_opus
        .or_else(|| model.clone())
        .or_else(|| small_fast.clone());

    if env.get("ANTHROPIC_DEFAULT_HAIKU_MODEL").is_none() {
        if let Some(v) = target_haiku {
            env.insert(
                "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                Value::String(v),
            );
            changed = true;
        }
    }
    if env.get("ANTHROPIC_DEFAULT_SONNET_MODEL").is_none() {
        if let Some(v) = target_sonnet {
            env.insert(
                "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                Value::String(v),
            );
            changed = true;
        }
    }
    if env.get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none() {
        if let Some(v) = target_opus {
            env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(), Value::String(v));
            changed = true;
        }
    }

    if env.remove("ANTHROPIC_SMALL_FAST_MODEL").is_some() {
        changed = true;
    }

    changed
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSortUpdate {
    pub id: String,
    #[serde(rename = "sortIndex")]
    pub sort_index: usize,
}

// ============================================================================
// 统一供应商（Universal Provider）服务方法
// ============================================================================

use crate::provider::UniversalProvider;
use std::collections::HashMap;

impl ProviderService {
    /// 获取所有统一供应商
    pub fn list_universal(
        state: &AppState,
    ) -> Result<HashMap<String, UniversalProvider>, AppError> {
        state.db.get_all_universal_providers()
    }

    /// 获取单个统一供应商
    pub fn get_universal(
        state: &AppState,
        id: &str,
    ) -> Result<Option<UniversalProvider>, AppError> {
        state.db.get_universal_provider(id)
    }

    /// 添加或更新统一供应商（不自动同步，需手动调用 sync_universal_to_apps）
    pub fn upsert_universal(
        state: &AppState,
        provider: UniversalProvider,
    ) -> Result<bool, AppError> {
        icon::validate_icon(provider.icon.as_deref())?;
        // 保存统一供应商
        state.db.save_universal_provider(&provider)?;

        Ok(true)
    }

    /// 删除统一供应商
    pub fn delete_universal(state: &AppState, id: &str) -> Result<bool, AppError> {
        // 获取统一供应商（用于删除生成的子供应商）
        let provider = state.db.get_universal_provider(id)?;

        // 删除统一供应商
        state.db.delete_universal_provider(id)?;

        // 删除生成的子供应商
        if let Some(p) = provider {
            if p.apps.claude {
                let claude_id = format!("universal-claude-{id}");
                let _ = state.db.delete_provider("claude", &claude_id);
            }
            if p.apps.codex {
                let codex_id = format!("universal-codex-{id}");
                let _ = state.db.delete_provider("codex", &codex_id);
            }
            if p.apps.gemini {
                let gemini_id = format!("universal-gemini-{id}");
                let _ = state.db.delete_provider("gemini", &gemini_id);
            }
        }

        Ok(true)
    }

    /// 同步统一供应商到各应用
    pub fn sync_universal_to_apps(state: &AppState, id: &str) -> Result<bool, AppError> {
        let provider = state
            .db
            .get_universal_provider(id)?
            .ok_or_else(|| AppError::Message(format!("统一供应商 {id} 不存在")))?;
        // 子供应商经 save_provider 直接入库，不走保存校验，图标在这里把关
        icon::validate_icon(provider.icon.as_deref())?;

        // 同步到 Claude
        if let Some(mut claude_provider) = provider.to_claude_provider() {
            // 合并已有配置
            if let Some(existing) = state.db.get_provider_by_id(&claude_provider.id, "claude")? {
                let mut merged = existing.settings_config.clone();
                Self::merge_json(&mut merged, &claude_provider.settings_config);
                claude_provider.settings_config = merged;
                // 已有子供应商的应用专属配置与排序不属于统一供应商管理的字段。
                claude_provider.meta = existing.meta;
                claude_provider.created_at = existing.created_at;
                claude_provider.sort_index = existing.sort_index;
            }
            state.db.save_provider("claude", &claude_provider)?;
        } else {
            // 如果禁用了 Claude，删除对应的子供应商
            let claude_id = format!("universal-claude-{id}");
            let _ = state.db.delete_provider("claude", &claude_id);
        }

        // 同步到 Codex
        if let Some(mut codex_provider) = provider.to_codex_provider() {
            // 合并已有配置
            if let Some(existing) = state.db.get_provider_by_id(&codex_provider.id, "codex")? {
                let mut merged = existing.settings_config.clone();
                Self::merge_json(&mut merged, &codex_provider.settings_config);
                codex_provider.settings_config = merged;
                // 已有子供应商的应用专属配置与排序不属于统一供应商管理的字段。
                codex_provider.meta = existing.meta;
                codex_provider.created_at = existing.created_at;
                codex_provider.sort_index = existing.sort_index;
            }
            state.db.save_provider("codex", &codex_provider)?;
        } else {
            let codex_id = format!("universal-codex-{id}");
            let _ = state.db.delete_provider("codex", &codex_id);
        }

        // 同步到 Gemini
        if let Some(mut gemini_provider) = provider.to_gemini_provider() {
            // 合并已有配置
            if let Some(existing) = state.db.get_provider_by_id(&gemini_provider.id, "gemini")? {
                let mut merged = existing.settings_config.clone();
                Self::merge_json(&mut merged, &gemini_provider.settings_config);
                gemini_provider.settings_config = merged;
                // 已有子供应商的应用专属配置与排序不属于统一供应商管理的字段。
                gemini_provider.meta = existing.meta;
                gemini_provider.created_at = existing.created_at;
                gemini_provider.sort_index = existing.sort_index;
            }
            state.db.save_provider("gemini", &gemini_provider)?;
        } else {
            let gemini_id = format!("universal-gemini-{id}");
            let _ = state.db.delete_provider("gemini", &gemini_id);
        }

        Ok(true)
    }

    /// 递归合并 JSON：base 为底，patch 覆盖同名字段
    fn merge_json(base: &mut serde_json::Value, patch: &serde_json::Value) {
        use serde_json::Value;

        match (base, patch) {
            (Value::Object(base_map), Value::Object(patch_map)) => {
                for (k, v_patch) in patch_map {
                    match base_map.get_mut(k) {
                        Some(v_base) => Self::merge_json(v_base, v_patch),
                        None => {
                            base_map.insert(k.clone(), v_patch.clone());
                        }
                    }
                }
            }
            // 其它类型：直接覆盖
            (base_val, patch_val) => {
                *base_val = patch_val.clone();
            }
        }
    }
}

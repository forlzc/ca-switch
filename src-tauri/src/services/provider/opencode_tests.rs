use super::{import_opencode_providers_from_live, ProviderService};
use crate::app_config::AppType;
use crate::database::Database;
use crate::opencode_config;
use crate::provider::Provider;
use crate::settings::{get_settings, update_settings, AppSettings};
use crate::store::AppState;
use serde_json::json;
use serial_test::serial;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

struct Fixture {
    previous_settings: AppSettings,
    previous_env: Vec<(&'static str, Option<OsString>)>,
    config_path: PathBuf,
    state: AppState,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous_env = ["CC_SWITCH_TEST_HOME", "OPENCODE_DB"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
        std::env::set_var("OPENCODE_DB", dir.path().join("opencode.db"));
        let previous_settings = get_settings();
        let config_dir = dir.path().join("opencode");
        fs::create_dir_all(&config_dir).unwrap();
        update_settings(AppSettings {
            opencode_config_dir: Some(config_dir.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .unwrap();
        Self {
            previous_settings,
            previous_env,
            config_path: config_dir.join("opencode.json"),
            state: AppState::new(Arc::new(Database::memory().unwrap())),
            _dir: dir,
        }
    }

    fn imported(&self) -> Provider {
        self.state
            .db
            .get_provider_by_id("opencode-go", "opencode")
            .unwrap()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Restore the cached settings while writes still target the temporary home.
        update_settings(self.previous_settings.clone()).unwrap();
        for (key, value) in &self.previous_env {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

const KEY_ONLY_CONFIG: &str = r#"{
  // Keep this comment and the user's formatting during import.
  "provider": {"opencode-go": {"options": {"apiKey": "{env:OPENCODE_API_KEY}"}}}
}"#;

#[test]
#[serial]
fn opencode_builtin_import_is_read_only_and_existing_override_is_editable() {
    let fixture = Fixture::new();
    fs::write(&fixture.config_path, KEY_ONLY_CONFIG).unwrap();
    let original_permissions = fs::metadata(&fixture.config_path).unwrap().permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&fixture.config_path, read_only).unwrap();

    let imported = import_opencode_providers_from_live(&fixture.state);
    fs::set_permissions(&fixture.config_path, original_permissions).unwrap();
    assert_eq!(imported.unwrap(), 1);
    assert_eq!(
        import_opencode_providers_from_live(&fixture.state).unwrap(),
        0
    );
    assert_eq!(
        fs::read_to_string(&fixture.config_path).unwrap(),
        KEY_ONLY_CONFIG
    );

    let mut provider = fixture.imported();
    assert!(provider.settings_config.get("npm").is_none());
    assert!(provider.settings_config["options"].get("baseURL").is_none());
    provider.settings_config["options"]["apiKey"] = json!("edited-test-key");
    ProviderService::update(&fixture.state, AppType::OpenCode, None, provider).unwrap();
    let saved = opencode_config::get_providers().unwrap();
    assert_eq!(saved["opencode-go"]["options"]["apiKey"], "edited-test-key");
    assert!(saved["opencode-go"].get("npm").is_none());
    assert!(saved["opencode-go"]["options"].get("baseURL").is_none());
}

#[test]
#[serial]
fn opencode_builtin_partial_model_override_can_omit_display_name() {
    let fixture = Fixture::new();
    opencode_config::set_provider(
        "opencode-go",
        json!({"models": {"glm-5": {"limit": {"context": 100000, "output": 10000}}}}),
    )
    .unwrap();
    assert_eq!(
        import_opencode_providers_from_live(&fixture.state).unwrap(),
        1
    );
    let mut provider = fixture.imported();
    assert!(provider.settings_config["models"]["glm-5"]
        .get("name")
        .is_none());
    provider.settings_config["options"]["apiKey"] = json!("edited-test-key");
    ProviderService::update(&fixture.state, AppType::OpenCode, None, provider).unwrap();
    let saved = opencode_config::get_providers().unwrap();
    assert!(saved["opencode-go"]["models"]["glm-5"]
        .get("name")
        .is_none());
    assert_eq!(
        saved["opencode-go"]["models"]["glm-5"]["limit"]["context"],
        100000
    );
}


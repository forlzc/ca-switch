use cc_switch_lib::{AppType, Provider, ProviderService};
use serde_json::json;
use std::{ffi::OsString, fs, path::PathBuf};

#[path = "support.rs"]
mod support;
use support::{create_test_state, reset_test_fs, test_mutex};

struct DshHome {
    directory: tempfile::TempDir,
    previous: Option<OsString>,
}
impl DshHome {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("DSH_HOME");
        std::env::set_var("DSH_HOME", directory.path());
        fs::create_dir_all(directory.path().join("profiles/web")).unwrap();
        Self {
            directory,
            previous,
        }
    }
    fn config(&self) -> PathBuf {
        self.directory.path().join("profiles/web/cordis.patch.yml")
    }
}
impl Drop for DshHome {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("DSH_HOME", value),
            None => std::env::remove_var("DSH_HOME"),
        }
    }
}
fn provider(id: &str) -> Provider {
    Provider::with_id(
        id.into(),
        id.into(),
        json!({"api":"openai-completions","options":{"baseURL":"https://example.com/v1","apiKey":"test-value"},"models":{"model":{"limit":{"context":100000}}}}),
        None,
    )
}

#[test]
fn dsh_provider_lifecycle_preserves_native_plugins_and_reusable_profiles() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let home = DshHome::new();
    let state = create_test_state().unwrap();
    let native = "# native\n- id: llm-pi-ai\n  config:\n    providers:\n      native: {api: openai-completions, baseURL: 'https://native.example/v1', apiKeyEnv: NATIVE_KEY, models: [{id: native-model}]}\n- id: other\n  config: {value: !!js process.env.USER_VALUE}\n";
    fs::write(home.config(), native).unwrap();
    let imported = ProviderService::list(&state, AppType::Dsh).unwrap();
    assert_eq!(
        imported["native"]
            .meta
            .as_ref()
            .unwrap()
            .live_config_managed,
        Some(true)
    );
    ProviderService::add(&state, AppType::Dsh, provider("draft"), false).unwrap();
    assert_eq!(fs::read_to_string(home.config()).unwrap(), native);
    ProviderService::switch(&state, AppType::Dsh, "draft").unwrap();
    let entries = ProviderService::list(&state, AppType::Dsh).unwrap();
    assert_eq!(
        entries["draft"].meta.as_ref().unwrap().live_config_managed,
        Some(true)
    );
    let mut edited = entries["draft"].clone();
    edited.settings_config["options"]["baseURL"] = json!("https://updated.example/v1");
    ProviderService::update(&state, AppType::Dsh, None, edited).unwrap();
    assert!(fs::read_to_string(home.config())
        .unwrap()
        .contains("https://updated.example/v1"));
    assert!(ProviderService::add(&state, AppType::Dsh, provider("native"), true).is_err());
    ProviderService::remove_from_live_config(&state, AppType::Dsh, "draft").unwrap();
    let entries = ProviderService::list(&state, AppType::Dsh).unwrap();
    assert_eq!(
        entries["draft"].meta.as_ref().unwrap().live_config_managed,
        Some(false)
    );
    assert!(fs::read_to_string(home.config())
        .unwrap()
        .contains("value: !!js process.env.USER_VALUE"));
    assert!(entries.contains_key("native"));
    ProviderService::delete(&state, AppType::Dsh, "draft").unwrap();
    assert!(state
        .db
        .get_provider_by_id("draft", "dsh")
        .unwrap()
        .is_none());
}

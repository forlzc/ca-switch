//! MCode TUI and desktop share this file. MCode owns model selection.
use crate::config::{atomic_write_private, get_home_dir};
use crate::error::AppError;
use indexmap::IndexMap;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) fn data_dir() -> PathBuf {
    explicit_data_dir(
        std::env::var("MINIMAX_DATA_DIR").ok().as_deref(),
        std::env::var("MAVIS_DATA_DIR").ok().as_deref(),
    )
    .unwrap_or_else(|| get_home_dir().join(".minimax"))
}

fn explicit_data_dir(minimax: Option<&str>, mavis: Option<&str>) -> Option<PathBuf> {
    [minimax, mavis]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|path| !path.is_empty())
        .map(PathBuf::from)
}

// Callers hold their feature lock across the native write and database commit.
pub(crate) fn write_and_commit<T>(
    path: &Path,
    write: impl FnOnce() -> Result<(), AppError>,
    commit: impl FnOnce() -> Result<T, AppError>,
) -> Result<T, AppError> {
    let previous = match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(AppError::io(path, error)),
    };
    write()?;
    match commit() {
        Ok(result) => Ok(result),
        Err(error) => {
            let rollback = match previous {
                Some(bytes) => atomic_write_private(path, &bytes),
                None => fs::remove_file(path).map_err(|error| AppError::io(path, error)),
            };
            if let Err(rollback_error) = rollback {
                return Err(AppError::Message(format!(
                    "Config update failed ({error}); restoring {} also failed: {rollback_error}",
                    path.display()
                )));
            }
            Err(error)
        }
    }
}

pub(crate) fn config_path() -> PathBuf {
    data_dir().join("config.yaml")
}

fn read(path: &Path) -> Result<serde_yaml::Value, AppError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(AppError::Message(format!(
                "Cannot read MCode configuration: {e}"
            )))
        }
    };
    let value: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|_| AppError::Config("Invalid MCode YAML configuration".into()))?;
    if value.is_null() {
        return Ok(serde_yaml::Value::Mapping(Default::default()));
    }
    if !value.is_mapping() {
        return Err(AppError::Config(
            "MCode configuration must be a mapping".into(),
        ));
    }
    Ok(value)
}

pub(crate) fn get_providers() -> Result<IndexMap<String, Value>, AppError> {
    let document = read(&config_path())?;
    match document.get("custom_provider") {
        None | Some(serde_yaml::Value::Null) => Ok(IndexMap::new()),
        Some(value) => {
            let mut providers: IndexMap<String, Value> = serde_yaml::from_value(value.clone())
                .map_err(|_| {
                    AppError::Config("Invalid MCode custom_provider configuration".into())
                })?;
            providers.retain(|_, provider| {
                provider
                    .get("kind")
                    .and_then(Value::as_str)
                    .is_none_or(|kind| kind == "custom")
            });
            Ok(providers)
        }
    }
}

/// Any `custom_provider` node under this key, including MCode's own account providers.
pub(crate) fn provider_key_exists(id: &str) -> Result<bool, AppError> {
    let document = read(&config_path())?;
    Ok(document
        .get("custom_provider")
        .and_then(serde_yaml::Value::as_mapping)
        .is_some_and(|providers| providers.contains_key(serde_yaml::Value::from(id))))
}

pub(crate) fn validate_provider(id: &str, config: &Value) -> Result<(), AppError> {
    if id.is_empty()
        || id
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_'))
    {
        return Err(AppError::InvalidInput(
            "MCode provider key must contain letters, digits, '-' or '_'".into(),
        ));
    }
    if !matches!(
        config
            .get("api")
            .map_or(Some("anthropic-messages"), Value::as_str),
        Some("anthropic-messages" | "openai-completions" | "openai-responses")
    ) {
        return Err(AppError::InvalidInput(
            "Select a supported MCode API format".into(),
        ));
    }
    let base_url = config
        .pointer("/options/baseURL")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !url::Url::parse(base_url)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
    {
        return Err(AppError::InvalidInput(
            "Enter a valid MCode endpoint URL".into(),
        ));
    }
    if config
        .pointer("/options/apiKey")
        .and_then(Value::as_str)
        .is_none_or(|key| key.trim().is_empty())
    {
        return Err(AppError::InvalidInput("Enter an API key".into()));
    }
    if config
        .get("models")
        .and_then(Value::as_object)
        .is_none_or(|models| models.is_empty() || models.keys().any(|id| id.trim().is_empty()))
    {
        return Err(AppError::InvalidInput(
            "Add at least one MCode model".into(),
        ));
    }
    Ok(())
}

// MCode's proper-lockfile uses the same atomic directory lock.
struct ConfigLock(PathBuf);
impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

fn update(path: &Path, id: &str, provider: Option<Value>) -> Result<(), AppError> {
    write_entry(path, id, provider, false)
}

/// `create_only` rejects a key that already exists, checked while holding MCode's lock.
fn write_entry(
    path: &Path,
    id: &str,
    provider: Option<Value>,
    create_only: bool,
) -> Result<(), AppError> {
    fs::create_dir_all(path.parent().expect("MCode configuration directory"))
        .map_err(|e| AppError::Message(format!("Cannot create MCode directory: {e}")))?;
    let path = if path.exists() {
        fs::canonicalize(path).map_err(|e| AppError::Message(e.to_string()))?
    } else {
        path.to_path_buf()
    };
    let lock_path = PathBuf::from(format!("{}.lock", path.display()));
    if let Err(error) = fs::create_dir(&lock_path) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err(AppError::io(&lock_path, error));
        }
        let stale = fs::metadata(&lock_path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|mtime| mtime.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(10));
        if !stale {
            return Err(AppError::Conflict(
                "MCode configuration is busy; retry after MCode finishes saving".into(),
            ));
        }
        fs::remove_dir(&lock_path).map_err(|e| AppError::io(&lock_path, e))?;
        fs::create_dir(&lock_path).map_err(|e| AppError::io(&lock_path, e))?;
    }
    let _lock = ConfigLock(lock_path);
    let mut document = read(&path)?;
    let prefix = format!("custom_provider:{id}/");
    if ["defaultModel", "defaultLightModel"].iter().any(|field| {
        let selected = document
            .get(*field)
            .and_then(serde_yaml::Value::as_str)
            .and_then(|model| model.strip_prefix(&prefix));
        selected.is_some_and(|model| {
            !provider.as_ref().is_some_and(|provider| {
                provider.get("enabled") != Some(&Value::Bool(false))
                    && provider
                        .get("models")
                        .and_then(|models| models.get(model))
                        .is_some_and(|model| {
                            model.is_object() && model.get("enabled") != Some(&Value::Bool(false))
                        })
            })
        })
    }) {
        return Err(AppError::InvalidInput(
            "Select another default model in MCode before removing this model or provider".into(),
        ));
    }
    let root = document.as_mapping_mut().expect("Validated mapping");
    let entry = root
        .entry("custom_provider".into())
        .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    if entry.is_null() {
        *entry = serde_yaml::Value::Mapping(Default::default());
    }
    let providers = entry
        .as_mapping_mut()
        .ok_or_else(|| AppError::Config("Invalid MCode custom_provider configuration".into()))?;
    if create_only && providers.contains_key(serde_yaml::Value::from(id)) {
        return Err(AppError::InvalidInput(format!(
            "MCode provider key '{id}' already exists"
        )));
    }
    if providers
        .get(serde_yaml::Value::from(id))
        .and_then(|value| value.get("kind"))
        .and_then(serde_yaml::Value::as_str)
        .is_some_and(|kind| kind != "custom")
    {
        return Err(AppError::InvalidInput(
            "MCode owns this account provider".into(),
        ));
    }
    if let Some(provider) = provider {
        providers.insert(
            id.into(),
            serde_yaml::to_value(provider)
                .map_err(|_| AppError::Config("Invalid MCode provider".into()))?,
        );
    } else {
        providers.remove(serde_yaml::Value::from(id));
    }
    let yaml = serde_yaml::to_string(&document)
        .map_err(|_| AppError::Config("Cannot serialize MCode configuration".into()))?;
    atomic_write_private(&path, yaml.as_bytes())
}

pub(crate) fn set_provider(id: &str, config: Value) -> Result<(), AppError> {
    validate_provider(id, &config)?;
    update(&config_path(), id, Some(config))
}
/// Like `set_provider`, but never replaces an existing node.
pub(crate) fn add_provider(id: &str, config: Value) -> Result<(), AppError> {
    validate_provider(id, &config)?;
    write_entry(&config_path(), id, Some(config), true)
}
pub(crate) fn remove_provider(id: &str) -> Result<(), AppError> {
    if !config_path().exists() {
        return Ok(());
    }
    update(&config_path(), id, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn native_data_directory_precedence_and_blank_values() {
        assert_eq!(
            explicit_data_dir(Some(" /primary "), Some("/legacy")),
            Some("/primary".into())
        );
        assert_eq!(
            explicit_data_dir(Some(" \t"), Some(" /legacy ")),
            Some("/legacy".into())
        );
        assert_eq!(explicit_data_dir(None, Some("")), None);
        assert_eq!(explicit_data_dir(None, None), None);
    }

    #[test]
    #[cfg(unix)]
    fn reclaims_a_stale_native_lock_but_preserves_an_active_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let lock = path.with_extension("yaml.lock");
        fs::create_dir(&lock).unwrap();
        assert!(update(&path, "test", Some(json!({}))).is_err());
        assert!(lock.exists());
        fs::File::open(&lock)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(11))
            .unwrap();
        update(&path, "test", Some(json!({"name":"Recovered"}))).unwrap();
        assert!(!lock.exists());
        assert_eq!(
            read(&path).unwrap()["custom_provider"]["test"]["name"],
            "Recovered"
        );
    }

    #[test]
    fn create_only_writes_never_replace_a_node_that_appeared_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let text = "custom_provider:\n  raced:\n    name: Theirs\n";
        fs::write(&path, text).unwrap();
        assert!(write_entry(&path, "raced", Some(json!({"name":"Ours"})), true).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert!(!path.with_extension("yaml.lock").exists());
        write_entry(&path, "fresh", Some(json!({"name":"Ours"})), true).unwrap();
        assert_eq!(
            read(&path).unwrap()["custom_provider"]["fresh"]["name"],
            "Ours"
        );
    }

    #[test]
    fn native_provider_can_omit_api_format() {
        let mut provider = json!({"options":{"baseURL":"https://example.com","apiKey":"test-key"},"models":{"model":{}}});
        validate_provider("native", &provider).unwrap();
        for api in [json!(null), json!(true), json!("unsupported")] {
            provider["api"] = api;
            assert!(validate_provider("native", &provider).is_err());
        }
    }
    #[test]
    fn additive_changes_preserve_other_providers_and_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        fs::write(&path, "defaultModel: minimax/MiniMax-M3\ncustom_provider:\n  existing:\n    name: Keep\nminimax_api:\n  apiKey: keep-secret\nunknown: [a, b]\n").unwrap();
        let before = read(&path).unwrap();
        let provider = json!({"api":"anthropic-messages","options":{"baseURL":"https://api.minimaxi.com/anthropic","apiKey":"test-key"},"models":{"MiniMax-M3":{}}});
        validate_provider("cc-switch-minimax", &provider).unwrap();
        update(&path, "cc-switch-minimax", Some(provider)).unwrap();
        let after = read(&path).unwrap();
        for field in ["defaultModel", "minimax_api", "unknown"] {
            assert_eq!(before[field], after[field]);
        }
        assert_eq!(
            before["custom_provider"]["existing"],
            after["custom_provider"]["existing"]
        );
        update(&path, "cc-switch-minimax", None).unwrap();
        assert_eq!(before, read(&path).unwrap());
    }
    #[test]
    fn respects_native_lock_and_selected_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let text = "defaultModel: custom_provider:chosen/model\ncustom_provider:\n  chosen: {}\n";
        fs::write(&path, text).unwrap();
        fs::create_dir(path.with_extension("yaml.lock")).unwrap();
        assert!(update(&path, "other", Some(json!({}))).is_err());
        fs::remove_dir(path.with_extension("yaml.lock")).unwrap();
        assert!(update(&path, "chosen", None).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), text);
    }
    #[test]
    fn preserves_default_and_light_model_availability() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        for field in ["defaultModel", "defaultLightModel"] {
            let text = format!("{field}: custom_provider:chosen/model\ncustom_provider:\n  chosen:\n    models:\n      model: {{}}\n");
            fs::write(&path, &text).unwrap();
            for replacement in [
                None,
                Some(json!({"models":{"other":{}}})),
                Some(json!({"enabled":false,"models":{"model":{}}})),
                Some(json!({"models":{"model":{"enabled":false}}})),
            ] {
                assert!(update(&path, "chosen", replacement).is_err());
                assert_eq!(fs::read_to_string(&path).unwrap(), text);
            }
            update(&path, "chosen", Some(json!({"models":{"model":{}}}))).unwrap();
            assert_eq!(read(&path).unwrap()[field], "custom_provider:chosen/model");
        }
    }
}

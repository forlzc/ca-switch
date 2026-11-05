//! Codex 会话压缩开关：`config.toml` 的 `[features] local_thread_store_compression`。
//!
//! 开启后 Codex 每次启动在后台把 7 天没动过的 rollout 压成 `.jsonl.zst`
//! （见 `codex_rollout_file`），会话管理与用量统计照常读取。
//! 开关状态以 `config.toml` 为准，不另存设置；这里只读不写。

use toml_edit::value;

use crate::codex_config::{codex_config_feature_enabled, read_codex_config_text};
use crate::error::AppError;
use crate::patch::toml::TomlPatch;
use crate::patch::KeyPath;
use crate::session_manager::providers::codex::session_roots;

const FEATURE_KEY: &str = "local_thread_store_compression";

fn feature_path() -> KeyPath {
    KeyPath::new(&["features", FEATURE_KEY])
}

pub fn is_enabled() -> Result<bool, AppError> {
    Ok(codex_config_feature_enabled(
        &read_codex_config_text()?,
        FEATURE_KEY,
    ))
}

fn patch(enabled: bool) -> TomlPatch {
    if enabled {
        TomlPatch {
            set: vec![(feature_path(), value(true))],
            ..TomlPatch::default()
        }
    } else {
        TomlPatch {
            remove: vec![feature_path()],
            ..TomlPatch::default()
        }
    }
}

/// Codex 会话目录（`sessions/` + `archived_sessions/`）当前占用的字节数。
pub fn sessions_disk_usage() -> u64 {
    session_roots()
        .iter()
        .map(|root| crate::services::backup_storage::path_size(root))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::LivePatch;
    use std::path::Path;

    fn apply(enabled: bool, before: &str) -> String {
        let bytes = patch(enabled)
            .apply(Path::new("config.toml"), Some(before.as_bytes()))
            .expect("apply patch");
        String::from_utf8(bytes).expect("utf8")
    }

    #[test]
    fn enabling_adds_key_and_keeps_layout() {
        let before = "# my config\nmodel = \"gpt-5\"\n\n[features]\nweb_search = true # keep\n";
        let after = apply(true, before);
        assert!(after.starts_with(
            "# my config\nmodel = \"gpt-5\"\n\n[features]\nweb_search = true # keep\n"
        ));
        assert!(codex_config_feature_enabled(&after, FEATURE_KEY));
        assert!(codex_config_feature_enabled(&after, "web_search"));
    }

    #[test]
    fn enabling_creates_features_table_in_empty_file() {
        let bytes = patch(true)
            .apply(Path::new("config.toml"), None)
            .expect("apply patch");
        let after = String::from_utf8(bytes).expect("utf8");
        assert!(codex_config_feature_enabled(&after, FEATURE_KEY));
    }

    #[test]
    fn disabling_removes_both_forms_and_keeps_other_features() {
        for before in [
            "[features]\nweb_search = true\nlocal_thread_store_compression = true\n",
            "[features]\nweb_search = true\nlocal_thread_store_compression = { enabled = true }\n",
        ] {
            let after = apply(false, before);
            assert!(
                !codex_config_feature_enabled(&after, FEATURE_KEY),
                "{after}"
            );
            assert!(!after.contains(FEATURE_KEY), "{after}");
            assert!(codex_config_feature_enabled(&after, "web_search"));
        }
    }

    #[test]
    fn table_form_counts_as_enabled() {
        assert!(codex_config_feature_enabled(
            "[features]\nlocal_thread_store_compression = { enabled = true }\n",
            FEATURE_KEY
        ));
        assert!(!codex_config_feature_enabled(
            "[features]\nlocal_thread_store_compression = false\n",
            FEATURE_KEY
        ));
    }
}

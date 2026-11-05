use std::path::PathBuf;

pub fn session_roots() -> Vec<PathBuf> {
    let config_dir = crate::grok_config::get_grok_config_dir();
    vec![
        config_dir.join("sessions"),
        config_dir.join("archived_sessions"),
    ]
}

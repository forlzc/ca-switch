use std::path::PathBuf;

use crate::app_config::AppType;
use crate::config::get_claude_settings_path;
use crate::error::AppError;

pub(crate) fn validate_prompt_content(_app: &AppType, _content: &str) -> Result<(), AppError> {
    Ok(())
}

/// 返回 API 应用所使用的提示词文件路径。
pub fn prompt_file_path(_app: &AppType) -> Result<PathBuf, AppError> {
    let base_dir = get_base_dir_with_fallback(get_claude_settings_path(), ".claude")?;
    Ok(base_dir.join("CLAUDE.md"))
}


fn get_base_dir_with_fallback(
    primary_path: PathBuf,
    fallback_dir: &str,
) -> Result<PathBuf, AppError> {
    primary_path
        .parent()
        .map(|p| p.to_path_buf())
        .or_else(|| dirs::home_dir().map(|h| h.join(fallback_dir)))
        .ok_or_else(|| {
            AppError::localized(
                "home_dir_not_found",
                format!("无法确定 {fallback_dir} 配置目录：用户主目录不存在"),
                format!("Cannot determine {fallback_dir} config directory: user home not found"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn prompt_file_uses_claude_md() {
        let path = prompt_file_path(&AppType::Claude).expect("prompt path");

        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("CLAUDE.md")
        );
    }
}

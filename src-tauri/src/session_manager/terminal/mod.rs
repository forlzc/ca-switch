/// 把任意字符串安全地包成 POSIX shell 的单引号字面量。
///
/// **必须是单引号**：双引号内 `$(...)`、反引号、`$VAR` 照常展开，而这里包的是
/// `projectDir`——会话历史里记录的真实项目路径，macOS 允许目录名含 `$` `(` `)`，
/// 所以一个名为 `$(...)` 的目录就足以让命令替换在用户终端里执行。
///
/// 单引号内不做任何展开，唯一的特例是 `'` 自身无法被表示：用「闭合-转义-重开」
/// 的 `'\''` 序列绕过。
pub(crate) fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_escape_neutralizes_command_substitution_in_directory_names() {
        assert_eq!(shell_escape("/tmp/$(id -un)"), "'/tmp/$(id -un)'");
        assert_eq!(shell_escape("/tmp/`id -un`"), "'/tmp/`id -un`'");
        assert_eq!(shell_escape("/tmp/$HOME"), "'/tmp/$HOME'");
    }

    #[test]
    fn shell_escape_handles_embedded_single_quote() {
        assert_eq!(shell_escape("/tmp/it's"), r"'/tmp/it'\''s'");
    }
}

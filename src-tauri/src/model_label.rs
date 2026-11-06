//! 发布给客户端的模型名与说明。
//!
//! 两件事：给模型 id 配一句说明（`<供应商名> · <上游模型名> · <窗口>`），以及把代际别名
//! 换成点版本对应的客户端 id（`claude-opus-5` → `claude-opus-5-5`）。

/// 代理给 Opus 档用的代际别名。
pub const PROXY_OPUS_ALIAS: &str = "claude-opus-5";
/// 代理给 Sonnet 档用的代际别名。
pub const PROXY_SONNET_ALIAS: &str = "claude-sonnet-5";

/// 点版本对应的客户端 id。
///
/// 客户端按 id 查自己的模型表决定显示名和输出上限：`claude-opus-5` 是「Opus 5」、
/// 64K 输出，`claude-opus-5-5` 才是「Opus 5.5」、128K。上游配的是 5.5 却写代际别名，
/// 菜单显示错、长输出被截断。上游 id 的分隔符各家不同（`claude-opus-5.5`、
/// `claude-opus-5-5`、`anthropic/claude-opus-5.5`、带日期的），按版本号匹配。
const POINT_RELEASE_CLIENT_IDS: &[(&str, &str, &str)] = &[
    (PROXY_OPUS_ALIAS, "opus-5", "claude-opus-5-5"),
    (PROXY_SONNET_ALIAS, "sonnet-5", "claude-sonnet-5-5"),
];

/// 上游是比代际别名更新的小版本时，客户端目录里对应的 id。
pub fn point_release_client_id(family_alias: &str, upstream: &str) -> Option<&'static str> {
    let id = upstream.trim().to_ascii_lowercase();
    POINT_RELEASE_CLIENT_IDS
        .iter()
        .filter(|(alias, _, _)| *alias == family_alias)
        .find(|(_, stem, _)| {
            [".5", "-5"].iter().any(|minor| {
                id.find(&format!("{stem}{minor}")).is_some_and(|at| {
                    let rest = &id[at + stem.len() + minor.len()..];
                    !rest.starts_with(|c: char| c.is_ascii_digit() || c == '.')
                })
            })
        })
        .map(|(_, _, client)| *client)
}

/// 模型说明：`<上游模型名> · <窗口>`。
pub fn model_description(model: &str, window: u64) -> String {
    match window_label(window) {
        Some(window) => format!("{model} · {window}"),
        None => model.to_string(),
    }
}

/// 模型说明：`<供应商名> · <上游模型名> · <窗口>`。供应商名为空时同 [`model_description`]。
pub fn provider_model_description(provider_name: &str, model: &str, window: u64) -> String {
    let description = model_description(model, window);
    match provider_name.trim() {
        "" => description,
        name => format!("{name} · {description}"),
    }
}

/// 窗口的简写：整百万写 `1M`，其余按千写 `256K`，不足一千照原样。
fn window_label(window: u64) -> Option<String> {
    match window {
        0 => None,
        w if w % 1_000_000 == 0 => Some(format!("{}M", w / 1_000_000)),
        w if w >= 1_000 => Some(format!("{}K", (w + 500) / 1_000)),
        w => Some(w.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_release_ids_replace_generation_aliases() {
        assert_eq!(
            point_release_client_id(PROXY_OPUS_ALIAS, "claude-opus-5.5"),
            Some("claude-opus-5-5")
        );
        assert_eq!(
            point_release_client_id(PROXY_SONNET_ALIAS, "anthropic/claude-sonnet-5-5-20260101"),
            Some("claude-sonnet-5-5")
        );
        // 5.50 不是 5.5：版本号后面紧跟数字就不算
        assert_eq!(
            point_release_client_id(PROXY_OPUS_ALIAS, "claude-opus-5.50"),
            None
        );
        // 4.x 不动
        assert_eq!(
            point_release_client_id(PROXY_OPUS_ALIAS, "claude-opus-4-6"),
            None
        );
    }

    #[test]
    fn descriptions_read_as_provider_model_window() {
        assert_eq!(
            provider_model_description("Relay", "gpt-5", 400_000),
            "Relay · gpt-5 · 400K"
        );
        assert_eq!(provider_model_description("", "gpt-5", 0), "gpt-5");
        assert_eq!(
            provider_model_description("Relay", "gpt-5", 1_000_000),
            "Relay · gpt-5 · 1M"
        );
    }
}

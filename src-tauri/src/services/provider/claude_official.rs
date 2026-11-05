//! Claude 官方卡绑定的账号。
//!
//! 官方卡可以在授权中心里绑一个 Claude 官方账号（`authBinding.authProvider = "claude_official"`，
//! `accountId` 是账号 key）。caswitch 不改写客户端的登录状态，这里只负责回答一个问题：
//! 从这张卡「打开终端」时，那个 `claude` 进程该用哪个账号目录。

use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use crate::services::claude_account::{self, snapshot};

/// 授权中心里 Claude 官方账号的认证供应商标识。
pub const AUTH_PROVIDER: &str = "claude_official";
pub const CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// Claude 官方卡（预设标了 `category: "official"`）。
pub fn is_official(provider: &Provider) -> bool {
    provider.category.as_deref() == Some("official")
}

/// 官方卡绑定的账号 key；没绑（默认登录）或不是官方卡时为 `None`。
pub fn bound_account(provider: &Provider) -> Option<String> {
    if !is_official(provider) {
        return None;
    }
    provider
        .meta
        .as_ref()?
        .managed_account_id_for(AUTH_PROVIDER)
        .filter(|id| !id.is_empty())
}

/// 账号在默认槽位、要用默认目录开终端时：当前激活的必须是官方卡，否则终端里的 `claude`
/// 会用默认目录里另一家的登录。
pub fn ensure_default_terminal_is_official(db: &Database) -> Result<(), AppError> {
    let app = crate::app_config::AppType::Claude;
    let in_use = crate::services::provider::ProviderService::active_provider(db, &app)?
        .filter(|provider| is_official(provider));
    if in_use.is_some() {
        return Ok(());
    }
    Err(AppError::localized(
        "claude.account.default_taken",
        "这个账号现在在 Claude Code 的默认登录里，但当前激活的不是官方卡，打开的终端不会用这个账号。请先切到这张卡（或任一官方卡）再打开终端",
        "This account is Claude Code's current sign-in, but the active provider is not an official card, so a new terminal wouldn't use this account. Switch to this card (or any official card) first, then open the terminal.",
    ))
}

/// 从卡片打开终端时，绑号卡的 `claude` 用哪里的登录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountTerminal {
    /// 账号自己的目录（`CLAUDE_CONFIG_DIR=<目录>`）
    Dir(std::path::PathBuf),
    /// 账号正在默认槽位里（切卡时移进去的，它自己的目录这时是空的）：用默认目录
    Default,
}

/// 绑了本机账号的官方卡打开终端时用哪里的登录（所有平台）；没绑为 `None`。
/// 绑的账号不在这台电脑上时报错，免得悄悄用错号。
pub fn terminal_account(provider: &Provider) -> Result<Option<AccountTerminal>, AppError> {
    let Some(account) = bound_account(provider) else {
        return Ok(None);
    };
    let dir = claude_account::config_dir_on_disk(&crate::config::get_app_config_dir(), &account);
    // 账号目录里有自己的登录就用它；没有、而这个号在默认槽位里（切卡时移进去的）才用默认目录。
    // 只看身份不够：默认目录和账号目录可能各登着同一个号（用户自己在 Claude Code 里登的）
    if let Some(dir) = dir.as_ref().filter(|dir| snapshot::dir_has_login(dir)) {
        return Ok(Some(AccountTerminal::Dir(dir.clone())));
    }
    if snapshot::is_live(&account) {
        return Ok(Some(AccountTerminal::Default));
    }
    match dir {
        Some(dir) => Ok(Some(AccountTerminal::Dir(dir))),
        None => Err(AppError::localized(
            "claude.account.not_on_device",
            format!(
                "「{}」绑定的 Claude 账号不在这台电脑上。请在授权中心添加它，或编辑卡片换一个账号",
                provider.name
            ),
            format!(
                "The Claude account bound to \"{}\" isn't on this computer. Add it in the Auth Center, or edit the card to choose another account.",
                provider.name
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta};
    use serde_json::json;

    fn official(binding: Option<&str>) -> Provider {
        let mut provider = Provider::with_id(
            "official".into(),
            "Claude Official".into(),
            json!({"env": {}}),
            None,
        );
        provider.category = Some("official".into());
        if let Some(account) = binding {
            provider.meta = Some(ProviderMeta {
                auth_binding: Some(AuthBinding {
                    source: AuthBindingSource::ManagedAccount,
                    auth_provider: Some(AUTH_PROVIDER.into()),
                    account_id: Some(account.into()),
                }),
                ..Default::default()
            });
        }
        provider
    }

    #[test]
    fn only_official_cards_have_bound_accounts() {
        assert_eq!(bound_account(&official(None)), None);
        assert_eq!(
            bound_account(&official(Some("acc:org"))).as_deref(),
            Some("acc:org")
        );
        let mut third_party = official(Some("acc:org"));
        third_party.category = Some("third_party".into());
        assert_eq!(bound_account(&third_party), None);
    }

    #[test]
    fn unbound_card_on_unknown_account_writes_nothing() {
        // 绑了但本机没有这个账号：打开终端报错，默认登录的卡不设目录
        assert!(terminal_account(&official(Some("missing:acc"))).is_err());
        assert!(terminal_account(&official(None)).unwrap().is_none());
    }
}

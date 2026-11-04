#![allow(dead_code)] // 测试辅助函数按需使用
use serde_json::json;

use cc_switch_lib::{
    read_json_file, write_codex_live_atomic, AppType, MultiAppConfig, Provider, ProviderService,
};

#[path = "support.rs"]
mod support;
use support::{
    create_test_state_with_config, enable_codex_official_auth_preservation, ensure_test_home,
    reset_test_fs, test_mutex,
};

fn sanitize_provider_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

#[test]
fn migrate_legacy_common_config_usage_marks_historical_provider_enabled() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    let _home = ensure_test_home();

    let mut config = MultiAppConfig::default();
    {
        let manager = config
            .get_manager_mut(&AppType::Claude)
            .expect("claude manager");
        manager.current = "legacy-provider".to_string();
        manager.providers.insert(
            "legacy-provider".to_string(),
            Provider::with_id(
                "legacy-provider".to_string(),
                "Legacy".to_string(),
                json!({
                    "includeCoAuthoredBy": false,
                    "env": {
                        "ANTHROPIC_API_KEY": "legacy-key"
                    }
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&config).expect("create test state");
    state
        .db
        .set_config_snippet(
            AppType::Claude.as_str(),
            Some(r#"{ "includeCoAuthoredBy": false }"#.to_string()),
        )
        .expect("set common config snippet");

    ProviderService::migrate_legacy_common_config_usage_if_needed(&state, AppType::Claude)
        .expect("migrate legacy common config");

    let providers = state
        .db
        .get_all_providers(AppType::Claude.as_str())
        .expect("get providers after migration");
    let provider = providers
        .get("legacy-provider")
        .expect("legacy provider exists");

    assert_eq!(
        provider
            .meta
            .as_ref()
            .and_then(|meta| meta.common_config_enabled),
        Some(true),
        "historical provider should be explicitly marked as using common config"
    );
    assert!(
        provider
            .settings_config
            .get("includeCoAuthoredBy")
            .is_none(),
        "common config fields should be stripped from provider storage after migration"
    );
    assert_eq!(
        provider
            .settings_config
            .get("env")
            .and_then(|v| v.get("ANTHROPIC_API_KEY"))
            .and_then(|v| v.as_str()),
        Some("legacy-key"),
        "provider-specific auth should remain untouched"
    );
}

#[test]
/// 第三方路由一律写成 `custom`：行里用什么 id 都一样，行本身不改写。
fn provider_service_switch_codex_writes_every_third_party_route_as_custom() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    let _home = ensure_test_home();

    let legacy_auth = json!({ "OPENAI_API_KEY": "rightcode-key" });
    let legacy_config = r#"model_provider = "rightcode"
model = "gpt-5.4"

[model_providers.rightcode]
name = "RightCode"
base_url = "https://rightcode.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;
    write_codex_live_atomic(&legacy_auth, Some(legacy_config))
        .expect("seed existing codex live config");

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "old-provider".to_string();
        manager.providers.insert(
            "old-provider".to_string(),
            Provider::with_id(
                "old-provider".to_string(),
                "RightCode".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "stale"},
                    "config": legacy_config
                }),
                None,
            ),
        );
        manager.providers.insert(
            "new-provider".to_string(),
            Provider::with_id(
                "new-provider".to_string(),
                "AiHubMix".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "fresh-key"},
                    "config": r#"model_provider = "aihubmix"
model = "gpt-5.4"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "new-provider")
        .expect("switch provider should succeed");

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    let parsed: toml::Value = toml::from_str(&config_text).expect("parse config.toml");

    assert_eq!(
        parsed.get("model_provider").and_then(|v| v.as_str()),
        Some("custom"),
        "every third-party route is written as the custom table"
    );

    let model_providers = parsed
        .get("model_providers")
        .and_then(|v| v.as_table())
        .expect("model_providers table exists");
    assert_eq!(
        model_providers
            .get("custom")
            .and_then(|v| v.get("base_url"))
            .and_then(|v| v.as_str()),
        Some("https://aihubmix.example/v1"),
        "the custom table points at the newly selected supplier endpoint"
    );
    assert!(
        model_providers.get("aihubmix").is_none(),
        "the row's own table id is not written"
    );
    assert!(
        model_providers.get("rightcode").is_none(),
        "the old version's table for old-provider (same id and address as its row) is retired"
    );

    let providers = state
        .db
        .get_all_providers(AppType::Codex.as_str())
        .expect("read providers after switch");
    let new_config_text = providers
        .get("new-provider")
        .expect("new provider exists")
        .settings_config
        .get("config")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        new_config_text.contains("[model_providers.aihubmix]"),
        "stored provider template stays as it was"
    );
}

#[test]
fn provider_service_switch_codex_preserves_oauth_and_keeps_rows_untouched() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    enable_codex_official_auth_preservation();
    let _home = ensure_test_home();

    let live_auth = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "access_token": "oauth-token",
            "account_id": "acct-1"
        }
    });
    let legacy_config = r#"model_provider = "rightcode"
model = "gpt-5.4"

[model_providers.rightcode]
name = "RightCode"
base_url = "https://rightcode.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;
    write_codex_live_atomic(&live_auth, Some(legacy_config))
        .expect("seed existing Codex OAuth live config");

    let bridge_provider = Provider::with_id(
        "bridge-provider".to_string(),
        "Bridge Provider".to_string(),
        json!({
            "auth": {"OPENAI_API_KEY": "bridge-key"},
            "config": r#"model_provider = "aihubmix"
model = "gpt-5.4"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
        }),
        None,
    );

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "legacy-provider".to_string();
        manager.providers.insert(
            "legacy-provider".to_string(),
            Provider::with_id(
                "legacy-provider".to_string(),
                "RightCode".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "rightcode-key"},
                    "config": legacy_config
                }),
                None,
            ),
        );
        manager
            .providers
            .insert("bridge-provider".to_string(), bridge_provider);
        manager.providers.insert(
            "plain-provider".to_string(),
            Provider::with_id(
                "plain-provider".to_string(),
                "Plain Provider".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "plain-key"},
                    "config": r#"model_provider = "plain"
model = "gpt-5.4"

[model_providers.plain]
name = "Plain"
base_url = "https://plain.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");
    let bridge_before = state
        .db
        .get_provider_by_id("bridge-provider", AppType::Codex.as_str())
        .expect("read bridge row")
        .expect("bridge row");

    ProviderService::switch(&state, AppType::Codex, "bridge-provider")
        .expect("switch to bridge provider should succeed");

    let auth_value: serde_json::Value =
        read_json_file(&cc_switch_lib::get_codex_auth_path()).expect("read auth.json");
    assert_eq!(
        auth_value.get("auth_mode").and_then(|v| v.as_str()),
        Some("chatgpt")
    );
    assert!(
        auth_value
            .get("OPENAI_API_KEY")
            .is_some_and(|v| v.is_null()),
        "provider switching should keep OPENAI_API_KEY null in live auth.json"
    );
    assert_eq!(
        auth_value
            .pointer("/tokens/access_token")
            .and_then(|v| v.as_str()),
        Some("oauth-token"),
        "existing ChatGPT OAuth token should be preserved"
    );

    let live_config =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    let parsed_live: toml::Value = toml::from_str(&live_config).expect("parse live config");
    assert_eq!(
        parsed_live.get("model_provider").and_then(|v| v.as_str()),
        Some("custom"),
        "third-party routes are always written as the custom table"
    );
    assert_eq!(
        parsed_live
            .get("model_providers")
            .and_then(|v| v.get("custom"))
            .and_then(|v| v.get("experimental_bearer_token"))
            .and_then(|v| v.as_str()),
        Some("bridge-key"),
        "third-party key should be injected into the selected live provider table"
    );
    assert_eq!(
        parsed_live
            .get("model_providers")
            .and_then(|v| v.get("custom"))
            .and_then(|v| v.get("requires_openai_auth"))
            .and_then(|v| v.as_bool()),
        Some(true)
    );

    // 旧版按行的 id 写进 live 的表（id 和地址都对得上 legacy-provider 的投影）被清掉，
    // 里面的真实 Key 不再留在 live 里。
    assert!(
        parsed_live
            .get("model_providers")
            .and_then(|v| v.get("rightcode"))
            .is_none(),
        "the table an old version wrote for legacy-provider is retired: {live_config}"
    );

    ProviderService::switch(&state, AppType::Codex, "plain-provider")
        .expect("switch away from the bridge provider");
    let bridge_after = state
        .db
        .get_provider_by_id("bridge-provider", AppType::Codex.as_str())
        .expect("read bridge row")
        .expect("bridge row");
    assert_eq!(
        bridge_after.settings_config, bridge_before.settings_config,
        "switching away never writes live content back into the row"
    );
}

#[allow(
    clippy::await_holding_lock,
    reason = "this integration-style test must serialize global test HOME and settings mutations across async takeover calls"
)]
async fn codex_official_to_deepseek_then_takeover_enters_and_restores_proxy_managed_live_config() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    enable_codex_official_auth_preservation();
    let _home = ensure_test_home();

    let oauth_auth = json!({
        "auth_mode": "chatgpt",
        "tokens": {
            "access_token": "oauth-access",
            "id_token": "oauth-id"
        }
    });
    let official_config = r#"model_provider = "openai"
model = "gpt-5"

[model_providers.openai]
name = "OpenAI"
wire_api = "responses"
"#;
    write_codex_live_atomic(&oauth_auth, Some(official_config))
        .expect("seed official Codex OAuth live config");

    let deepseek_provider_config = r#"model_provider = "deepseek"
model = "deepseek-chat"

[model_providers.deepseek]
name = "DeepSeek"
base_url = "https://api.deepseek.com/v1"
wire_api = "responses"
"#;
    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "official-provider".to_string();

        let mut official_provider = Provider::with_id(
            "official-provider".to_string(),
            "OpenAI Official".to_string(),
            json!({
                "auth": oauth_auth,
                "config": official_config
            }),
            None,
        );
        official_provider.category = Some("official".to_string());
        manager
            .providers
            .insert("official-provider".to_string(), official_provider);

        let mut deepseek_provider = Provider::with_id(
            "deepseek-provider".to_string(),
            "DeepSeek".to_string(),
            json!({
                "auth": {"OPENAI_API_KEY": "deepseek-key"},
                "config": deepseek_provider_config
            }),
            None,
        );
        deepseek_provider.category = Some("custom".to_string());
        manager
            .providers
            .insert("deepseek-provider".to_string(), deepseek_provider);
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    let mut proxy_config = state.db.get_proxy_config().await.expect("get proxy config");
    proxy_config.listen_port = 0;
    state
        .db
        .update_proxy_config(proxy_config)
        .await
        .expect("use ephemeral proxy port");

    ProviderService::switch(&state, AppType::Codex, "deepseek-provider")
        .expect("switch from official subscription to DeepSeek");

    let auth_after_switch: serde_json::Value =
        read_json_file(&cc_switch_lib::get_codex_auth_path()).expect("read auth after switch");
    assert_eq!(
        auth_after_switch, oauth_auth,
        "normal provider switch with Codex preservation enabled must keep OAuth auth.json"
    );

    let config_after_switch =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config");
    assert!(
        config_after_switch.contains("https://api.deepseek.com/v1"),
        "normal switch should write the DeepSeek endpoint before takeover"
    );
    assert!(
        config_after_switch.contains("deepseek-key"),
        "normal switch should inject the DeepSeek key into config.toml"
    );

    cc_switch_lib::mode::controller::enter(&state, &AppType::Codex, false)
        .await
        .expect("enter Codex routing mode");
    let proxy_status = state
        .proxy_service
        .get_status()
        .await
        .expect("read proxy status after takeover");
    let codex_proxy_base_url = format!("http://127.0.0.1:{}/v1", proxy_status.port);

    let auth_after_takeover: serde_json::Value =
        read_json_file(&cc_switch_lib::get_codex_auth_path()).expect("read auth after takeover");
    assert_eq!(
        auth_after_takeover, oauth_auth,
        "enabling takeover must not rewrite Codex OAuth auth.json"
    );

    let config_after_takeover =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config");
    assert!(
        config_after_takeover.contains(&codex_proxy_base_url),
        "enabling takeover should point Codex config.toml at the local proxy"
    );
    assert!(
        config_after_takeover.contains("PROXY_MANAGED"),
        "enabling takeover should move the proxy placeholder into config.toml"
    );
    assert!(
        !config_after_takeover.contains("https://api.deepseek.com/v1"),
        "takeover live config should not keep the upstream DeepSeek endpoint"
    );

    cc_switch_lib::mode::controller::exit(&state, &AppType::Codex)
        .await
        .expect("leave Codex routing mode");

    let restored_auth: serde_json::Value =
        read_json_file(&cc_switch_lib::get_codex_auth_path()).expect("read restored auth");
    assert_eq!(
        restored_auth, oauth_auth,
        "disabling takeover should restore without replacing OAuth auth.json"
    );

    let restored_config = std::fs::read_to_string(cc_switch_lib::get_codex_config_path())
        .expect("read restored config");
    assert!(
        restored_config.contains("https://api.deepseek.com/v1")
            && restored_config.contains("deepseek-key"),
        "disabling takeover should restore the selected DeepSeek live config"
    );
    assert!(
        !restored_config.contains("PROXY_MANAGED"),
        "restored live config must not keep the proxy placeholder"
    );
}

#[test]
fn provider_service_switch_codex_default_removes_auth_json_when_preservation_off() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // Intentionally do NOT enable preservation: this locks the default opt-out
    // behavior where a third-party switch deletes auth.json outright — the
    // official OAuth login is not preserved, and the third-party key never
    // lands there either (it travels as the provider-scoped bearer token in
    // config.toml). It is the dual of
    // `provider_service_switch_codex_preserves_oauth_and_backfills_api_key_from_live_token`.
    let _home = ensure_test_home();

    let live_auth = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "access_token": "official-oauth-token",
            "account_id": "acct-1"
        }
    });
    let legacy_config = r#"model_provider = "rightcode"
model = "gpt-5.4"

[model_providers.rightcode]
name = "RightCode"
base_url = "https://rightcode.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;
    write_codex_live_atomic(&live_auth, Some(legacy_config))
        .expect("seed existing Codex OAuth live config");

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "legacy-provider".to_string();
        manager.providers.insert(
            "legacy-provider".to_string(),
            Provider::with_id(
                "legacy-provider".to_string(),
                "RightCode".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "rightcode-key"},
                    "config": legacy_config
                }),
                None,
            ),
        );
        manager.providers.insert(
            "third-party".to_string(),
            Provider::with_id(
                "third-party".to_string(),
                "AiHubMix".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "third-party-key"},
                    "config": r#"model_provider = "aihubmix"
model = "gpt-5.4"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "third-party")
        .expect("switch to third-party provider should succeed");

    assert!(
        !cc_switch_lib::get_codex_auth_path().exists(),
        "default (preservation off) must delete auth.json on a third-party switch — \
         the official login goes away and the key rides in config.toml instead"
    );
    let live_config =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        live_config.contains("experimental_bearer_token = \"third-party-key\""),
        "the third-party key must be injected as the provider-scoped bearer token; got:\n{live_config}"
    );
}

#[test]
fn provider_service_switch_codex_default_injects_bearer_token_into_config() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // Preservation stays OFF (default). Since Codex 0.149 (openai/codex#39214)
    // custom providers no longer inherit ambient auth, so third-party switches
    // are config-only on every path: the key travels as a provider-scoped
    // `experimental_bearer_token` and auth.json is removed.
    let _home = ensure_test_home();

    let third_party_config = r#"model_provider = "aihubmix"
model = "gpt-5.4"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
requires_openai_auth = false
"#;

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "third-party".to_string(),
            Provider::with_id(
                "third-party".to_string(),
                "AiHubMix".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "third-party-key"},
                    "config": third_party_config
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "third-party")
        .expect("switch to third-party provider should succeed");

    assert!(
        !cc_switch_lib::get_codex_auth_path().exists(),
        "third-party switches are config-only: no auth.json is written"
    );

    let live_config =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        live_config.contains("experimental_bearer_token = \"third-party-key\""),
        "default switch must inject the API key into config.toml so Codex >= 0.149 \
         custom providers authenticate (openai/codex#39214); got:\n{live_config}"
    );
}

#[test]
fn provider_service_switch_codex_preserved_login_rejects_empty_third_party_config() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // Preservation ON + third-party provider with an empty config: auth.json is
    // not written, and an empty config.toml has no provider table to carry the
    // bearer token, so the API key has nowhere to land while the official
    // OAuth login stays live — Codex would silently fall back to the official
    // provider and bill the ChatGPT account. The switch must be refused, as it
    // was before the bearer-token injection change.
    let _home = ensure_test_home();
    enable_codex_official_auth_preservation();

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "empty-config".to_string(),
            Provider::with_id(
                "empty-config".to_string(),
                "EmptyConfig".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "third-party-key"},
                    "config": ""
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    let err = ProviderService::switch(&state, AppType::Codex, "empty-config").expect_err(
        "switching to an empty-config third-party provider with preservation on must fail",
    );
    assert!(
        err.to_string().contains("config.toml"),
        "error should explain the missing config.toml, got: {err}"
    );
}

#[test]
fn provider_service_switch_codex_preserved_login_normalizes_legacy_reroute_config() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // Preservation ON + a legacy-shape third-party config (top-level
    // openai_base_url rerouting the built-in `openai` provider): the shape
    // has no provider table to carry the bearer token — since 0.149 the
    // built-in provider would keep using the preserved official OAuth from
    // auth.json and send it to the third-party base URL. The switch must
    // normalize the config into the custom table with the key
    // injected, leaving the official login untouched.
    let _home = ensure_test_home();
    enable_codex_official_auth_preservation();

    let live_auth = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "access_token": "official-oauth-token",
            "account_id": "acct-1"
        }
    });
    write_codex_live_atomic(&live_auth, Some("")).expect("seed official OAuth live config");

    let legacy_shape_config = r#"model_provider = "openai"
model = "gpt-5.4"
openai_base_url = "https://relay.example/v1"
"#;

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "legacy-shape".to_string(),
            Provider::with_id(
                "legacy-shape".to_string(),
                "LegacyShape".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "third-party-key"},
                    "config": legacy_shape_config
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "legacy-shape")
        .expect("legacy reroute shape must be normalized, not rejected");

    let live_config =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_config.contains("openai_base_url"),
        "the top-level reroute must be rewritten away; got:\n{live_config}"
    );
    assert!(
        live_config.contains("[model_providers.custom]")
            && live_config.contains("base_url = \"https://relay.example/v1\"")
            && live_config.contains("experimental_bearer_token = \"third-party-key\""),
        "routing and key must move into the custom provider table; got:\n{live_config}"
    );

    let auth_value: serde_json::Value =
        read_json_file(&cc_switch_lib::get_codex_auth_path()).expect("read auth.json");
    assert_eq!(
        auth_value
            .pointer("/tokens/access_token")
            .and_then(|v| v.as_str()),
        Some("official-oauth-token"),
        "the preserved official OAuth login must stay untouched"
    );
}

#[test]
fn provider_service_switch_codex_preserved_login_normalizes_config_carried_token() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // Same legacy reroute shape, but the key sits in the config text itself
    // (raw-edited provider with `auth = {}`): normalization must see
    // config-carried tokens too, not only auth.OPENAI_API_KEY, and the
    // injected token must land inside the rewritten provider table.
    let _home = ensure_test_home();
    enable_codex_official_auth_preservation();

    let raw_edited_config = r#"model_provider = "openai"
model = "gpt-5.4"
openai_base_url = "https://relay.example/v1"
experimental_bearer_token = "config-carried-key"
"#;

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "raw-edited".to_string(),
            Provider::with_id(
                "raw-edited".to_string(),
                "RawEdited".to_string(),
                json!({
                    "auth": {},
                    "config": raw_edited_config
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "raw-edited")
        .expect("legacy reroute with a config-carried token must be normalized");

    let live_config =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_config.contains("openai_base_url"),
        "the top-level reroute must be rewritten away; got:\n{live_config}"
    );
    assert!(
        live_config.contains("[model_providers.custom]"),
        "the custom provider table must be created; got:\n{live_config}"
    );
    assert_eq!(
        cc_switch_lib::extract_codex_experimental_bearer_token(&live_config).as_deref(),
        Some("config-carried-key"),
        "the config-carried key must resolve for the rewritten provider; got:\n{live_config}"
    );
}

#[test]
fn provider_service_switch_codex_default_normalizes_legacy_reroute_config() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // Same legacy shape with preservation OFF (default): the switch is
    // config-only on every path, so instead of feeding the built-in
    // provider's ambient auth through auth.json the shape is normalized into
    // a custom table and auth.json is removed.
    let _home = ensure_test_home();

    let legacy_shape_config = r#"model_provider = "openai"
model = "gpt-5.4"
openai_base_url = "https://relay.example/v1"
"#;

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "legacy-shape".to_string(),
            Provider::with_id(
                "legacy-shape".to_string(),
                "LegacyShape".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "third-party-key"},
                    "config": legacy_shape_config
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "legacy-shape")
        .expect("default-path switch must normalize the legacy ambient-auth shape");

    assert!(
        !cc_switch_lib::get_codex_auth_path().exists(),
        "third-party switches are config-only: no auth.json is written"
    );
    let live_config =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_config.contains("openai_base_url")
            && live_config.contains("[model_providers.custom]")
            && live_config.contains("experimental_bearer_token = \"third-party-key\""),
        "routing and key must move into the custom provider table; got:\n{live_config}"
    );
}

#[test]
fn provider_service_switch_codex_preserved_login_rejects_keyless_official_auth_fallback() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // Preservation ON + a header-auth card with NO API key anywhere
    // (`auth = {}`) whose config also sets `requires_openai_auth = true`:
    // there is no token to inject, so Codex 0.149 resolves auth from the
    // preserved official OAuth in auth.json and applies it AFTER provider
    // headers — the explicit Authorization header is overwritten and the
    // ChatGPT access token + account id go to the third-party endpoint.
    // The switch must be refused (fail closed).
    let _home = ensure_test_home();
    enable_codex_official_auth_preservation();

    let header_auth_with_fallback = r#"model_provider = "custom"
model = "gpt-5.4"

[model_providers.custom]
name = "Custom"
base_url = "https://relay.example/v1"
wire_api = "responses"
requires_openai_auth = true
http_headers = { Authorization = "Bearer explicit-header-token" }
"#;

    let good_config = r#"model_provider = "good"
model = "gpt-5.4"

[model_providers.good]
name = "Good"
base_url = "https://good.example/v1"
wire_api = "responses"
"#;

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "good".to_string(),
            Provider::with_id(
                "good".to_string(),
                "Good".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "sk-good"},
                    "config": good_config
                }),
                None,
            ),
        );
        manager.providers.insert(
            "header-auth".to_string(),
            Provider::with_id(
                "header-auth".to_string(),
                "HeaderAuth".to_string(),
                json!({
                    "auth": {},
                    "config": header_auth_with_fallback
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "good").expect("switch to the good provider");

    ProviderService::switch(&state, AppType::Codex, "header-auth").expect_err(
        "preservation-on switch must fail when a keyless config falls back to the official auth",
    );

    // The refusal happens in the pre-commit preflight: current must not move,
    // otherwise the next switch would backfill the good provider's live
    // config into the refused card's DB row.
    let current = state
        .db
        .get_current_provider(AppType::Codex.as_str())
        .expect("read current provider");
    assert_eq!(
        current.as_deref(),
        Some("good"),
        "a refused switch must leave current on the previous provider"
    );
}

#[test]
fn provider_service_switch_codex_preserved_login_allows_keyless_header_auth_provider() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // Same keyless header-auth card WITHOUT the fallback flag: 0.149 resolves
    // this provider as unauthenticated, provider headers survive untouched,
    // and the third-party key in http_headers.Authorization does the auth.
    // This legitimate shape must keep switching under preservation.
    let _home = ensure_test_home();
    enable_codex_official_auth_preservation();

    let header_auth_config = r#"model_provider = "custom"
model = "gpt-5.4"

[model_providers.custom]
name = "Custom"
base_url = "https://relay.example/v1"
wire_api = "responses"
http_headers = { Authorization = "Bearer explicit-header-token" }
"#;

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.providers.insert(
            "header-auth".to_string(),
            Provider::with_id(
                "header-auth".to_string(),
                "HeaderAuth".to_string(),
                json!({
                    "auth": {},
                    "config": header_auth_config
                }),
                None,
            ),
        );
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "header-auth")
        .expect("preservation-on switch must keep supporting keyless header-auth providers");

    let config_text =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        config_text.contains("Authorization = \"Bearer explicit-header-token\""),
        "the provider's own Authorization header must be written verbatim; got:\n{config_text}"
    );
    assert!(
        !config_text.contains("experimental_bearer_token"),
        "no token exists, nothing must be injected; got:\n{config_text}"
    );
}

#[test]
fn provider_service_switch_codex_supports_official_login_provider_without_auth_write() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    let _home = ensure_test_home();

    let live_auth = json!({
        "auth_mode": "chatgpt",
        "OPENAI_API_KEY": null,
        "tokens": {
            "access_token": "official-oauth-token",
            "account_id": "acct-official"
        }
    });
    write_codex_live_atomic(&live_auth, Some("")).expect("seed official OAuth live config");

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "legacy-provider".to_string();
        manager.providers.insert(
            "legacy-provider".to_string(),
            Provider::with_id(
                "legacy-provider".to_string(),
                "Legacy".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "legacy-key"},
                    "config": r#"model_provider = "legacy"

[model_providers.legacy]
name = "Legacy"
base_url = "https://legacy.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#
                }),
                None,
            ),
        );
        let official_provider = Provider::with_id(
            "codex-official".to_string(),
            "OpenAI Official".to_string(),
            json!({
                "auth": {},
                "config": ""
            }),
            None,
        );
        manager
            .providers
            .insert("codex-official".to_string(), official_provider);
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "codex-official")
        .expect("switch to official provider should succeed without API key");

    let auth_value: serde_json::Value =
        read_json_file(&cc_switch_lib::get_codex_auth_path()).expect("read auth.json");
    assert_eq!(
        auth_value.get("auth_mode").and_then(|v| v.as_str()),
        Some("chatgpt")
    );
    assert!(
        auth_value
            .get("OPENAI_API_KEY")
            .is_some_and(|v| v.is_null()),
        "official provider switching should keep OPENAI_API_KEY null"
    );
    assert_eq!(
        auth_value
            .pointer("/tokens/access_token")
            .and_then(|v| v.as_str()),
        Some("official-oauth-token"),
        "official provider should preserve the existing ChatGPT OAuth token"
    );

    let live_config =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_config.contains("experimental_bearer_token"),
        "official login provider has no API key to inject"
    );
}

#[test]
fn provider_service_switch_codex_official_clears_stale_third_party_auth() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    // preservation stays OFF (default). Versions before the key moved into
    // config.toml wrote the third-party key into live auth.json; that residue
    // is what this test expects the official switch to clean up.
    let _home = ensure_test_home();

    let third_party_config = r#"model_provider = "aihubmix"
model = "gpt-5.4"

[model_providers.aihubmix]
name = "AiHubMix"
base_url = "https://aihubmix.example/v1"
wire_api = "responses"
requires_openai_auth = true
"#;
    // The residue is the row's own key: only a key some third-party row
    // carries is provably CC Switch's to delete (a user's own
    // `codex login --api-key` login is left alone).
    let live_auth = json!({ "OPENAI_API_KEY": "old-db-key" });
    write_codex_live_atomic(&live_auth, Some(third_party_config))
        .expect("seed third-party live config");

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "third-party".to_string();
        manager.providers.insert(
            "third-party".to_string(),
            Provider::with_id(
                "third-party".to_string(),
                "AiHubMix".to_string(),
                json!({
                    "auth": {"OPENAI_API_KEY": "old-db-key"},
                    "config": third_party_config
                }),
                None,
            ),
        );
        let mut official_provider = Provider::with_id(
            "official-provider".to_string(),
            "OpenAI Official".to_string(),
            json!({
                "auth": {},
                "config": ""
            }),
            None,
        );
        official_provider.category = Some("official".to_string());
        manager
            .providers
            .insert("official-provider".to_string(), official_provider);
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "official-provider")
        .expect("switch to official provider should succeed");

    assert!(
        !cc_switch_lib::get_codex_auth_path().exists(),
        "switching to a material-less official provider must delete the stale \
         third-party auth.json so Codex shows its login screen"
    );

    let providers = state
        .db
        .get_all_providers(AppType::Codex.as_str())
        .expect("read providers after switch");
    assert_eq!(
        providers
            .get("third-party")
            .expect("third-party provider exists")
            .settings_config
            .pointer("/auth/OPENAI_API_KEY")
            .and_then(|v| v.as_str()),
        Some("old-db-key"),
        "the outgoing row is never rewritten from live"
    );

    let live_config =
        std::fs::read_to_string(cc_switch_lib::get_codex_config_path()).expect("read config.toml");
    assert!(
        !live_config.contains("experimental_bearer_token"),
        "official provider has no API key to inject"
    );
}

#[test]
fn provider_service_reswitch_current_official_keeps_live_auth() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    let _home = ensure_test_home();

    // Re-selecting the already-current provider performs no backfill, so the
    // cleanup must not run either: without a fresh DB copy of whatever sits
    // in live auth.json, deleting it would destroy the only copy.
    let live_auth = json!({ "OPENAI_API_KEY": "residue-key" });
    write_codex_live_atomic(&live_auth, Some("")).expect("seed live config");

    let mut initial_config = MultiAppConfig::default();
    {
        let manager = initial_config
            .get_manager_mut(&AppType::Codex)
            .expect("codex manager");
        manager.current = "official-provider".to_string();
        let mut official_provider = Provider::with_id(
            "official-provider".to_string(),
            "OpenAI Official".to_string(),
            json!({
                "auth": {},
                "config": ""
            }),
            None,
        );
        official_provider.category = Some("official".to_string());
        manager
            .providers
            .insert("official-provider".to_string(), official_provider);
    }

    let state = create_test_state_with_config(&initial_config).expect("create test state");

    ProviderService::switch(&state, AppType::Codex, "official-provider")
        .expect("re-switch to current official provider should succeed");

    let auth_value: serde_json::Value =
        read_json_file(&cc_switch_lib::get_codex_auth_path()).expect("auth.json must survive");
    assert_eq!(
        auth_value.get("OPENAI_API_KEY").and_then(|v| v.as_str()),
        Some("residue-key"),
        "no backfill happened, so live auth.json must be left untouched"
    );
}

#[test]
fn read_codex_live_settings_tolerates_missing_auth_when_config_file_exists() {
    let _guard = test_mutex().lock().unwrap_or_else(|e| e.into_inner());
    reset_test_fs();
    let _home = ensure_test_home();

    assert!(
        cc_switch_lib::read_codex_live_settings().is_err(),
        "both files missing is still 'no live install'"
    );

    // auth.json deleted + empty config.toml is the exact state the official
    // switch cleanup leaves behind; it must stay readable or the next
    // backfill / hot switch would treat Codex as uninstalled.
    let config_path = cc_switch_lib::get_codex_config_path();
    std::fs::create_dir_all(config_path.parent().expect("codex dir")).expect("create codex dir");
    std::fs::write(&config_path, "").expect("write empty config.toml");

    let live = cc_switch_lib::read_codex_live_settings()
        .expect("config file present but empty must be readable");
    assert_eq!(live.get("auth"), Some(&json!({})));
    assert_eq!(live.get("config").and_then(|v| v.as_str()), Some(""));
}

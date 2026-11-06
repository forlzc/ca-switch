mod app_config;
mod app_store;
mod auto_launch;
mod commands;
mod config;
mod database;
mod deeplink;
mod error;
mod init_status;
 
mod lightweight;
#[cfg(target_os = "linux")]
mod linux_fix;
mod model_capabilities;
mod panic_hook;
pub mod floor;
pub mod model_label;
pub mod protocol;
mod provider;
mod proxy;
mod services;
mod settings;
mod store;

mod tray;
#[cfg(target_os = "windows")]
mod tray_compat;
mod tray_panel;
mod usage_events;
mod usage_script;

pub use app_config::{AppType, MultiAppConfig};
pub use commands::open_provider_terminal;
pub use commands::*;
pub use config::{get_claude_settings_path, read_json_file};
pub use database::{Database, Profile};
pub use deeplink::{import_provider_from_deeplink, parse_deeplink_url, DeepLinkImportRequest};
pub use error::AppError;
pub use protocol::{AuthStyle, Conversion, Matrix, Protocol, ProtocolSource, Transform};
pub use provider::{Provider, ProviderMeta};
pub use services::{
    profile::{ProfilePayload, ProfileScope, ProfileService},
    provider::EditorSave,
    ConfigService, EndpointLatency, ProviderService, ProxyService, SpeedtestService,
};
pub use settings::{update_settings, AppSettings};
pub use store::AppState;
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use std::sync::atomic::{AtomicBool, Ordering};
use std::{fmt, sync::Arc};
use tauri::tray::TrayIconBuilder;
use tauri::RunEvent;
use tauri::{Emitter, Manager};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

#[cfg(target_os = "windows")]
fn set_windows_app_user_model_id(app: &tauri::AppHandle) {
    let app_id = app.config().identifier.clone();
    let wide_app_id: Vec<u16> = app_id.encode_utf16().chain(std::iter::once(0)).collect();

    let result = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(wide_app_id.as_ptr())
    };

    if result < 0 {
        log::warn!("设置 Windows AppUserModelID 失败: 0x{result:08X}");
    } else {
        log::debug!("Windows AppUserModelID 已设置为 {app_id}");
    }
}

#[cfg(target_os = "windows")]
fn disable_windows_general_autofill<R: tauri::Runtime>(webview: &tauri::Webview<R>) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings4;
    use windows::core::Interface;

    let result = webview.with_webview(|platform_webview| {
        let result = unsafe {
            platform_webview
                .controller()
                .CoreWebView2()
                .and_then(|webview| webview.Settings())
                .and_then(|settings| settings.cast::<ICoreWebView2Settings4>())
                .and_then(|settings| settings.SetIsGeneralAutofillEnabled(false))
        };

        if let Err(error) = result {
            log::warn!("禁用 Windows WebView2 常规自动填充失败: {error}");
        }
    });

    if let Err(error) = result {
        log::warn!("访问 Windows WebView2 失败: {error}");
    }
}

pub(crate) struct RedactedUrl<'a> {
    url: &'a str,
    known_secrets: &'a [String],
}

impl fmt::Display for RedactedUrl<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&redact_url_for_log_with_secrets(
            self.url,
            self.known_secrets,
        ))
    }
}

/// 为日志提供惰性 URL 脱敏包装；只有日志实际输出时才解析和重建 URL。
pub(crate) fn url_for_log(url: &str) -> RedactedUrl<'_> {
    RedactedUrl {
        url,
        known_secrets: &[],
    }
}

/// 为持有确切认证材料的调用方提供优先精确匹配、再启发式兜底的 URL 脱敏。
pub(crate) fn url_for_log_with_secrets<'a>(
    url: &'a str,
    known_secrets: &'a [String],
) -> RedactedUrl<'a> {
    RedactedUrl { url, known_secrets }
}

/// 已知密钥参与子串脱敏的最短长度：过短的值(如 "api")当作子串会误伤无关文本，
/// 所以只对足够长、几乎不可能是普通词的值做替换。
const MIN_KNOWN_SECRET_LEN: usize = 8;

/// 唯一的密钥脱敏原语：把字符串里出现的、我们确切握有的密钥值替换为 [REDACTED]。
/// 不做任何“看起来像密钥”的形状猜测——只隐藏已知值，天然收敛、不误伤正常路径。
pub(crate) fn redact_known_secrets(text: &str, known_secrets: &[String]) -> String {
    redact_known_secrets_with_min_length(text, known_secrets, MIN_KNOWN_SECRET_LEN)
}

/// Exact-value redaction for user-visible error text. Unlike URL logging, a
/// short known credential must still be hidden because the result reaches the
/// UI rather than a diagnostic-only path.
pub(crate) fn redact_known_secrets_strict(text: &str, known_secrets: &[String]) -> String {
    redact_known_secrets_with_min_length(text, known_secrets, 1)
}

fn redact_known_secrets_with_min_length(
    text: &str,
    known_secrets: &[String],
    minimum_chars: usize,
) -> String {
    let mut output = text.to_string();
    for secret in known_secrets {
        if secret.chars().count() >= minimum_chars {
            output = output.replace(secret.as_str(), "[REDACTED]");
        }
    }
    output
}

/// 无 scheme 的裸 authority 形态(如 `user:pass@host/path`)剥掉 userinfo：
/// 仅当 `@` 出现在第一个 `/` 之前时才视为凭据。
fn strip_bare_userinfo(input: &str) -> &str {
    let authority_end = input.find('/').unwrap_or(input.len());
    match input[..authority_end].rfind('@') {
        Some(at) => &input[at + 1..],
        None => input,
    }
}

pub(crate) fn redact_url_for_log(url_str: &str) -> String {
    redact_url_for_log_with_secrets(url_str, &[])
}

/// 为日志脱敏 URL：剥掉 userinfo(user:pass@) 与整个 query/fragment，保留
/// scheme/host/port/path 供诊断(如 base_url 配错路径导致 404)，最后再抹掉已知密钥值。
pub(crate) fn redact_url_for_log_with_secrets(url_str: &str, known_secrets: &[String]) -> String {
    let scheme_relative = url_str.starts_with("//");
    let parsed = if scheme_relative {
        url::Url::parse(&format!("https:{url_str}"))
    } else {
        url::Url::parse(url_str)
    };

    let sanitized = match parsed {
        Ok(mut url) if url.has_host() => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            let rendered = url.as_str();
            if scheme_relative {
                rendered
                    .strip_prefix("https:")
                    .unwrap_or(rendered)
                    .to_string()
            } else {
                rendered.to_string()
            }
        }
        _ => {
            // 解析失败(相对路径、含裸 userinfo 的非法 URL 等)：丢掉 query/fragment，
            // 尽力剥掉 userinfo，其余原样保留。
            let without_tail = url_str.split(['?', '#']).next().unwrap_or(url_str);
            strip_bare_userinfo(without_tail).to_string()
        }
    };

    redact_known_secrets(&sanitized, known_secrets)
}

/// 只保留 `scheme://host:port`，丢掉 path/query/userinfo。用于我们手里没有任何
/// 已知密钥可脱敏 path 的场景——凭据可能整个内嵌在 base_url 的 path 里，此时
/// 记录 path 无法保证不泄漏，只能退回到 origin。
pub(crate) fn redact_url_origin_for_log(url_str: &str) -> String {
    let scheme_relative = url_str.starts_with("//");
    let parsed = if scheme_relative {
        url::Url::parse(&format!("https:{url_str}"))
    } else {
        url::Url::parse(url_str)
    };

    match parsed {
        Ok(url) if url.has_host() => {
            let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
            if scheme_relative {
                format!("//{authority}")
            } else {
                format!("{}://{authority}", url.scheme())
            }
        }
        _ => "[invalid target]".to_string(),
    }
}

/// 给日志用的错误文本：去掉 TOML 解析诊断里引用的源码行（`1 | key = "..."` 和它上下的
/// `|`、`^` 标注行）。那一行是用户配置原文，出错的可能正是密钥那一行；行列号和原因留着。
pub(crate) fn error_for_log(error: &str) -> String {
    error
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start()
                .starts_with('|')
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn runtime_log_level_allows(level: log::Level, max_level: log::LevelFilter) -> bool {
    max_level.to_level().is_some_and(|maximum| level <= maximum)
}

/// 统一处理 ccswitch:// 深链接 URL
///
/// - 解析 URL，把结果（或错误）暂存为待处理深链接，再发 `deeplink-pending` 通知前端来读
///   （前端可能还没挂载，见 `deeplink::PendingDeepLink`）
/// - 可选：聚焦主窗口
fn handle_deeplink_url(
    app: &tauri::AppHandle,
    url_str: &str,
    focus_main_window: bool,
    source: &str,
) -> bool {
    if !url_str.starts_with("ccswitch://") {
        return false;
    }

    log::info!(
        "✓ Deep link URL detected from {source}: {}",
        url_for_log(url_str)
    );

    let pending = match crate::deeplink::parse_deeplink_url(url_str) {
        Ok(request) => {
            log::info!(
                "✓ Successfully parsed deep link: resource={}, app={:?}, name={:?}",
                request.resource,
                request.app,
                request.name
            );
            crate::deeplink::PendingDeepLink::Import {
                request: Box::new(request),
            }
        }
        Err(e) => {
            log::error!("✗ Failed to parse deep link URL: {e}");
            crate::deeplink::PendingDeepLink::Error {
                url: url_str.to_string(),
                error: e.to_string(),
            }
        }
    };
    crate::deeplink::set_pending_deeplink(pending);

    if let Err(e) = app.emit("deeplink-pending", ()) {
        log::error!("✗ Failed to emit deeplink-pending event: {e}");
    }

    if focus_main_window {
        if let Some(window) = app.get_webview_window("main") {
            #[cfg(target_os = "windows")]
            {
                let _ = window.set_skip_taskbar(false);
            }
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
            #[cfg(target_os = "linux")]
            {
                linux_fix::nudge_main_window(window.clone(), "deeplink");
            }
            log::info!("✓ Window shown and focused");
        }
    }

    true
}

/// 更新托盘菜单的Tauri命令
#[tauri::command]
async fn update_tray_menu(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    if !tray_panel::uses_native_menu() {
        // 用托盘面板、不挂原生菜单：更新图标状态，通知开着的面板重新取数据。
        tray::refresh_tray_menu(&app);
        return Ok(app.tray_by_id(tray::TRAY_ID).is_some());
    }
    match tray::create_tray_menu(&app, state.inner()) {
        Ok(new_menu) => {
            if let Some(tray) = app.tray_by_id(tray::TRAY_ID) {
                tray.set_menu(Some(new_menu))
                    .map_err(|e| format!("更新托盘菜单失败: {e}"))?;
                return Ok(true);
            }
            Ok(false)
        }
        Err(err) => {
            log::error!("创建托盘菜单失败: {err}");
            Ok(false)
        }
    }
}

/// 本进程打开文件描述符的软上限目标。macOS 的 `setrlimit` 不接受超过 `OPEN_MAX`（10240）的值。
#[cfg(unix)]
const OPEN_FILE_SOFT_LIMIT_TARGET: libc::rlim_t = 10240;

/// 从 Finder / 登录项启动的 macOS 应用继承 launchd 的软上限，默认只有 256 个文件描述符。
/// 本地路由并发时客户端连接、上游连接、日志、数据库和配置文件都要占用描述符，突发请求下
/// accept、上游连接和打开数据库会一起报 EMFILE（Too many open files）。启动时把软上限
/// 提到硬上限（封顶 10240），硬上限更低或已经足够时不动。
#[cfg(unix)]
fn raise_open_file_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: 传入的是有效的 rlimit 指针，调用只读写这个结构体。
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        log::warn!(
            "读取文件描述符上限失败: {}",
            std::io::Error::last_os_error()
        );
        return;
    }
    let target = OPEN_FILE_SOFT_LIMIT_TARGET.min(limit.rlim_max);
    if limit.rlim_cur >= target {
        return;
    }
    let previous = limit.rlim_cur;
    limit.rlim_cur = target;
    // SAFETY: 同上；软上限不超过硬上限，不需要特权。
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) } != 0 {
        log::warn!(
            "提高文件描述符上限失败（当前 {previous}）: {}",
            std::io::Error::last_os_error()
        );
    } else {
        log::info!("文件描述符软上限 {previous} → {target}");
    }
}

/// 又启动了一个实例（或经命令行打开 ccswitch:// 链接）：退出轻量模式、处理链接、把主窗口拿到前台。
/// 在后台线程上调用（见 single_instance 回调），一次处理一个：连着启动两次时，
/// 不能两边都看到主窗口不在、各建一个。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn handle_second_instance(app: &tauri::AppHandle, args: Vec<String>) {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    log::debug!("Args count: {}", args.len());
    for (i, arg) in args.iter().enumerate() {
        log::debug!("  arg[{i}]: {}", url_for_log(arg));
    }

    if crate::lightweight::is_lightweight_mode() {
        if let Err(e) = crate::lightweight::exit_lightweight_mode(app) {
            log::error!("退出轻量模式重建窗口失败: {e}");
        }
    }

    // Check for deep link URL in args (mainly for Windows/Linux command line)
    let mut found_deeplink = false;
    for arg in &args {
        if handle_deeplink_url(app, arg, false, "single_instance args") {
            found_deeplink = true;
            break;
        }
    }

    if !found_deeplink {
        log::info!(
            "ℹ No deep link URL found in args (this is expected on macOS when launched via system)"
        );
    }

    // Show and focus window regardless
    if let Some(window) = app.get_webview_window("main") {
        // 防御性重置 Windows 的 skip_taskbar：single_instance 触发时，
        // 原进程可能因 silent_startup / 关闭到托盘等处于 skip_taskbar(true) 状态，
        // 仅 show() 不会重置该状态，会导致窗口可见但不在任务栏、最小化后消失。
        #[cfg(target_os = "windows")]
        {
            let _ = window.set_skip_taskbar(false);
        }
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        #[cfg(target_os = "linux")]
        {
            linux_fix::nudge_main_window(window.clone(), "single-instance");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 设置 panic hook，在应用崩溃时记录日志到 <app_config_dir>/crash.log（默认 ~/.cc-switch/crash.log）
    panic_hook::setup_panic_hook();

    let mut builder = tauri::Builder::default();

    #[cfg(target_os = "windows")]
    {
        builder = builder.plugin(
            tauri::plugin::Builder::<_, ()>::new("windows-general-autofill")
                .on_webview_ready(|webview| {
                    if webview.label() == "main" {
                        disable_windows_general_autofill(&webview);
                    }
                })
                .build(),
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            log::info!("=== Single Instance Callback Triggered ===");
            // Windows 上这个回调跑在主线程处理 WM_COPYDATA 的窗口过程里，第二个实例的
            // SendMessage 正等它返回：轻量模式下要在这里同步重建主窗口，WebView2 会把主线程
            // 卡死（托盘随之无响应）。挪到后台线程，建窗口改由事件循环处理，和 macOS / Linux
            // 本来就不在主线程上调用一致。
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || handle_second_instance(&app, args));
        }));
    }

    #[cfg(target_os = "windows")]
    {
        let startup_page_handled = AtomicBool::new(false);
        builder = builder.on_page_load(move |webview, payload| {
            if webview.label() == "main"
                && payload.event() == tauri::webview::PageLoadEvent::Finished
                && payload.url().scheme() != "about"
                && !startup_page_handled.swap(true, Ordering::Relaxed)
                && !crate::settings::get_settings().silent_startup
            {
                let _ = webview.window().show();
                log::info!("主页面加载完成，主窗口已显示");
            }
        });
    }

    let builder = builder
        // 注册 deep-link 插件（处理 macOS AppleEvent 和其他平台的深链接）
        .plugin(tauri_plugin_deep_link::init())
        // 拦截窗口关闭：根据设置决定是否最小化到托盘
        .on_window_event(|window, event| {
            // 托盘面板自己处理关闭（只隐藏），不走主窗口的「最小化到托盘 / 退出」
            if window.label() == tray_panel::PANEL_LABEL {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                log::info!("收到窗口关闭请求: label={}", window.label());
                // 数据库版本过新的恢复模式下没有托盘可唤回，关闭即退出，避免应用隐身后台
                let in_db_recovery = crate::init_status::get_init_error()
                    .map(|p| p.kind.as_deref() == Some("db_version_too_new"))
                    .unwrap_or(false);
                if in_db_recovery {
                    api.prevent_close();
                    window.app_handle().exit(0);
                    return;
                }

                let settings = crate::settings::get_settings();

                if settings.minimize_to_tray_on_close {
                    api.prevent_close();
                    let _ = window.hide();
                    log::info!("关闭请求已处理：最小化到托盘");
                    #[cfg(target_os = "windows")]
                    {
                        let _ = window.set_skip_taskbar(true);
                    }
                    #[cfg(target_os = "macos")]
                    {
                        tray::apply_tray_policy(window.app_handle(), false);
                    }
                } else {
                    api.prevent_close();
                    log::info!("关闭请求已处理：退出应用");
                    window.app_handle().exit(0);
                }
            }
        })
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(window_state_flags())
                .with_denylist(&[tray_panel::PANEL_LABEL])
                .build(),
        )
        .setup(|app| {
            let _ = rustls::crypto::ring::default_provider().install_default();

            // 预先刷新 Store 覆盖配置，确保后续路径读取正确（日志/数据库等）
            app_store::refresh_app_config_dir_override(app.handle());
            panic_hook::init_app_config_dir(crate::config::get_app_config_dir());

            // 初始化日志（输出到 <app_config_dir>/logs/cc-switch.log）
            {
                use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};

                let log_dir = panic_hook::get_log_dir();

                // 确保日志目录存在
                if let Err(e) = std::fs::create_dir_all(&log_dir) {
                    eprintln!("创建日志目录失败: {e}");
                }

                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        // 底层保留 Trace 能力，便于加载用户配置后动态调高级别。
                        // 插件注册后会立即把全局级别收紧到 Info，避免启动阶段全量 Trace。
                        .level(log::LevelFilter::Trace)
                        // plugin-log 的前端 command 会直达 logger，绕过 log 宏的全局
                        // max_level；在分发层补一次过滤，确保动态总开关同样约束前端日志。
                        .filter(|metadata| {
                            runtime_log_level_allows(metadata.level(), log::max_level())
                        })
                        .targets([
                            Target::new(TargetKind::Stdout),
                            Target::new(TargetKind::Folder {
                                path: log_dir,
                                file_name: Some("ca-switch".into()),
                            }),
                        ])
                        // KeepSome(4) 保留 4 个轮转归档，加上当前文件最多约 100 MiB。
                        // 轮转仅按大小触发；跨重启继续追加，不再丢失上一次运行的日志。
                        .rotation_strategy(RotationStrategy::KeepSome(4))
                        .max_file_size(20 * 1024 * 1024)
                        .timezone_strategy(TimezoneStrategy::UseLocal)
                        .build(),
                )?;

                // 用户配置存在数据库中，数据库尚未打开时使用保守的 Info 级别。
                log::set_max_level(log::LevelFilter::Info);
                log::info!("=== CC Switch v{} started ===", env!("CARGO_PKG_VERSION"));
            }

            #[cfg(unix)]
            raise_open_file_limit();

            // 首次读取覆盖路径时 logger 尚未可用；此处重放一次，
            // 让 Store 损坏或路径无效等启动警告能够真正落盘。
            let _ = app_store::refresh_app_config_dir_override(app.handle());

            #[cfg(target_os = "windows")]
            set_windows_app_user_model_id(app.handle());

            // 注册 Updater 插件（桌面端）；放在 logger 之后，确保失败可诊断。
            #[cfg(desktop)]
            {
                if let Err(e) = app
                    .handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())
                {
                    // 若配置不完整（如缺少 pubkey），跳过 Updater 而不中断应用
                    log::warn!("初始化 Updater 插件失败，已跳过：{e}");
                }
            }

            // 注入 AppHandle 给 usage_events，让无 AppHandle 持有的写日志路径
            // 也能向前端推送 `usage-log-recorded`。
            // 放在日志系统初始化之后，确保 init 的日志能正常输出。
            usage_events::init(app.handle().clone());

            // 初始化数据库
            let app_config_dir = crate::config::get_app_config_dir();
            let db_path = app_config_dir.join("ca-switch.db");
            let json_path = app_config_dir.join("config.json");

            // 检查是否需要从 config.json 迁移到 SQLite
            let has_json = json_path.exists();
            let has_db = db_path.exists();

            // 如果需要迁移，先验证 config.json 是否可以加载（在创建数据库之前）
            // 这样如果加载失败用户选择退出，数据库文件还没被创建，下次可以正常重试
            let migration_config = if !has_db && has_json {
                log::info!("检测到旧版配置文件，验证配置文件...");

                // 循环：支持用户重试加载配置文件
                loop {
                    match crate::app_config::MultiAppConfig::load() {
                        Ok(config) => {
                            log::info!("✓ 配置文件加载成功");
                            break Some(config);
                        }
                        Err(e) => {
                            log::error!("加载旧配置文件失败: {e}");
                            // 弹出系统对话框让用户选择
                            if !show_migration_error_dialog(app.handle(), &e.to_string()) {
                                // 用户选择退出（此时数据库还没创建，下次启动可以重试）
                                log::info!("用户选择退出程序");
                                std::process::exit(1);
                            }
                            // 用户选择重试，继续循环
                            log::info!("用户选择重试加载配置文件");
                        }
                    }
                }
            } else {
                None
            };

            // 现在创建数据库（包含 Schema 迁移）
            //
            // 说明：从 v3.8.* 升级的用户通常会走到这里的 SQLite schema 迁移，
            // 若迁移失败（数据库损坏/权限不足/user_version 过新等），需要给用户明确提示，
            // 否则表现可能只是“应用打不开/闪退”。
            //
            // 预检：数据库版本过新时，必须先于任何 schema 写操作（create_tables 内含
            // DROP/ALTER 等 DDL）进入恢复界面，避免旧应用对读不懂的更新版 DB 落写。
            match crate::database::Database::stored_user_version_exceeds_supported(&db_path) {
                Ok(Some(version)) => {
                    log::warn!("数据库版本过新（v{version}），引导用户在应用内升级应用");
                    crate::init_status::set_init_error(crate::init_status::InitErrorPayload {
                        path: db_path.display().to_string(),
                        error: format!(
                            "数据库版本过新（{version}），当前应用仅支持 {}，请升级应用后再尝试。",
                            crate::database::SCHEMA_VERSION
                        ),
                        kind: Some("db_version_too_new".to_string()),
                        db_version: Some(version),
                        supported_version: Some(crate::database::SCHEMA_VERSION),
                    });
                    // 主窗口默认 visible:false，恢复界面必须强制显示
                    if let Some(window) = app.get_webview_window("main") {
                        #[cfg(target_os = "windows")]
                        {
                            let _ = window.set_skip_taskbar(false);
                        }
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                    return Ok(());
                }
                Ok(None) => {}
                Err(e) => {
                    log::warn!("预检数据库版本失败，继续正常初始化流程: {e}");
                }
            }

            let db = loop {
                match crate::database::Database::init() {
                    Ok(db) => break Arc::new(db),
                    Err(e) => {
                        log::error!("Failed to init database: {e}");

                        if !show_database_init_error_dialog(app.handle(), &db_path, &e.to_string())
                        {
                            log::info!("用户选择退出程序");
                            std::process::exit(1);
                        }

                        log::info!("用户选择重试初始化数据库");
                    }
                }
            };

            // 数据库可用后立即应用持久化日志级别，避免后续服务初始化
            // 继续使用启动阶段的 Info 回退。损坏配置显式 fail-closed 到 Info。
            match db.get_log_config() {
                Ok(log_config) => {
                    log::set_max_level(log_config.to_level_filter());
                    log::info!(
                        "已加载日志配置: enabled={}, level={}",
                        log_config.enabled,
                        log_config.level
                    );
                }
                Err(e) => {
                    log::set_max_level(log::LevelFilter::Info);
                    log::warn!("读取日志配置失败，已回退到 info: {e}");
                }
            }

            // 如果有预加载的配置，执行迁移
            if let Some(config) = migration_config {
                log::info!("开始执行数据迁移...");

                match db.migrate_from_json(&config) {
                    Ok(_) => {
                        log::info!("✓ 配置迁移成功");
                        // 标记迁移成功，供前端显示 Toast
                        crate::init_status::set_migration_success();
                        // 归档旧配置文件（重命名而非删除，便于用户恢复）
                        let archive_path = json_path.with_extension("json.migrated");
                        if let Err(e) = std::fs::rename(&json_path, &archive_path) {
                            log::warn!("归档旧配置文件失败: {e}");
                        } else {
                            log::info!("✓ 旧配置已归档为 config.json.migrated");
                        }
                    }
                    Err(e) => {
                        // 配置加载成功但迁移失败的情况极少（磁盘满等），仅记录日志
                        log::error!("配置迁移失败: {e}，将从现有配置导入");
                    }
                }
            }

            let app_state = AppState::new(db);

            // 设置 AppHandle 用于代理故障转移时的 UI 更新
            app_state.proxy_service.set_app_handle(app.handle().clone());


            // ============================================================
            // 按表独立判断的导入逻辑（各类数据独立检查，互不影响）
            // ============================================================

            // 1.5. 不再写入官方预设供应商：精简版只允许用户新建自定义供应商。
            //
            // 捕获首次运行快照：所有全新装用户都会看到欢迎弹窗介绍本应用的工作方式。
            // 读失败时默认不弹，宁可漏弹也不要因为故障打扰用户。
            let first_run_already_confirmed = crate::settings::get_settings()
                .first_run_notice_confirmed
                .unwrap_or(false);
            let fresh_install_at_startup =
                app_state.db.is_providers_empty().unwrap_or(false);

            // 老用户 / 已确认的路径由 `fresh_install_at_startup` 自行拦截，这里不做写入。
            // 字段只由前端在用户点击"我知道了"时 save_settings 回写，语义是"用户显式确认过"。
            if !first_run_already_confirmed && fresh_install_at_startup {
                log::info!("✓ First-run welcome notice pending");
            }

            // 精简版升级：一次性清空历史供应商数据，之后只允许新建自定义供应商。
            let reset_done = app_state
                .db
                .get_setting("slim_provider_reset_v1")
                .ok()
                .flatten()
                .is_some();
            if !reset_done {
                match app_state.db.delete_all_providers() {
                    Ok(()) => {
                        log::info!("✓ 精简版升级：已清空历史供应商数据");
                    }
                    Err(e) => log::warn!("✗ 清空历史供应商数据失败: {e}"),
                }
                let _ = app_state.db.set_setting("slim_provider_reset_v1", "true");
            }

            // 提示词功能已移除，不再导入提示词文件。

            // 迁移旧的 app_config_dir 配置到 Store
            if let Err(e) = app_store::migrate_app_config_dir_from_settings(app.handle()) {
                log::warn!("迁移 app_config_dir 失败: {e}");
            }

            // 启动阶段不再无条件保存,避免意外覆盖用户配置。

            // 注册 deep-link URL 处理器（使用正确的 DeepLinkExt API）
            log::info!("=== Registering deep-link URL handler ===");

            // Linux 和 Windows 调试模式需要显式注册
            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            {
                #[cfg(target_os = "linux")]
                {
                    // Use Tauri's path API to get correct path (includes app identifier)
                    // tauri-plugin-deep-link writes to: ~/.local/share/com.ca-switch.desktop/applications/ca-switch-handler.desktop
                    // Only register if .desktop file doesn't exist to avoid overwriting user customizations
                    let should_register = app
                        .path()
                        .data_dir()
                        .map(|d| !d.join("applications/ca-switch-handler.desktop").exists())
                        .unwrap_or(true);

                    if should_register {
                        if let Err(e) = app.deep_link().register_all() {
                            log::error!("✗ Failed to register deep link schemes: {}", e);
                        } else {
                            log::info!("✓ Deep link schemes registered (Linux)");
                        }
                    } else {
                        log::info!("⊘ Deep link handler already exists, skipping registration");
                    }
                }

                #[cfg(all(debug_assertions, windows))]
                {
                    if let Err(e) = app.deep_link().register_all() {
                        log::error!("✗ Failed to register deep link schemes: {}", e);
                    } else {
                        log::info!("✓ Deep link schemes registered (Windows debug)");
                    }
                }
            }

            // 注册 URL 处理回调（所有平台通用）
            app.deep_link().on_open_url({
                let app_handle = app.handle().clone();
                move |event| {
                    log::info!("=== Deep Link Event Received (on_open_url) ===");
                    let urls = event.urls();
                    log::info!("Received {} URL(s)", urls.len());

                    if crate::lightweight::is_lightweight_mode() {
                        if let Err(e) = crate::lightweight::exit_lightweight_mode(&app_handle) {
                            log::error!("退出轻量模式重建窗口失败: {e}");
                        }
                    }

                    for (i, url) in urls.iter().enumerate() {
                        let url_str = url.as_str();
                        log::debug!("  URL[{i}]: {}", url_for_log(url_str));

                        if handle_deeplink_url(&app_handle, url_str, true, "on_open_url") {
                            break; // Process only first ccswitch:// URL
                        }
                    }
                }
            });
            log::info!("✓ Deep-link URL handler registered");

            // 冷启动补取：Windows/Linux 上插件在自身初始化时就解析了启动参数并发出事件，
            // macOS 的 Opened 也可能早于这里，都赶在上面的回调注册之前，只能从 get_current 取。
            // 万一两边都处理了同一条链接也无妨：暂存区只留最新一条，前端处理完就清除。
            let mut startup_deeplink = false;
            match app.deep_link().get_current() {
                Ok(Some(urls)) => {
                    for url in urls {
                        if handle_deeplink_url(app.handle(), url.as_str(), true, "get_current") {
                            startup_deeplink = true;
                            break;
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => log::warn!("读取启动时的深链接失败: {e}"),
            }

            // 构建托盘：Windows 左键打开主界面，右键默认弹出面板；macOS 左右键默认都弹出面板。
            // 设置里打开「原生托盘菜单」就挂原生菜单（`tray::apply_tray_style` 运行时切换）。
            // Linux（AppIndicator）不派发点击事件，只能出原生菜单，菜单里有一项打开面板。
            let native_menu = tray_panel::uses_native_menu();
            let mut tray_builder = TrayIconBuilder::with_id(tray::TRAY_ID)
                .tooltip("CC Switch") // 鼠标悬停提示
                .on_tray_icon_event(tray::handle_tray_icon_event)
                .on_menu_event(|app, event| {
                    tray::handle_tray_menu_event(app, &event.id.0);
                })
                .show_menu_on_left_click(tray::native_menu_on_left_click(native_menu));
            if native_menu {
                let menu = tray::create_tray_menu(app.handle(), &app_state)?;
                tray_builder = tray_builder.menu(&menu);
            }

            // 使用平台对应的托盘图标（macOS 使用模板图标适配深浅色）；出问题时 tray.rs 换成带圆点的那张
            if let Some((icon, template)) = tray::base_tray_icon(app.handle()) {
                tray_builder = tray_builder.icon(icon).icon_as_template(template);
            }

            // Linux 托盘需要把图标写入磁盘。WSL 的 XDG_RUNTIME_DIR 可能指向
            // 不存在或不可写的目录，使用应用缓存目录避免因此中止启动。
            #[cfg(target_os = "linux")]
            {
                tray_builder =
                    tray_builder.temp_dir_path(app.path().app_cache_dir()?.join("tray-icon"));
            }

            let _tray = tray_builder.build(app)?;
            #[cfg(target_os = "windows")]
            tray_compat::install(app.handle());
            crate::services::webdav_auto_sync::start_worker(
                app_state.db.clone(),
                app.handle().clone(),
            );
            crate::services::s3_auto_sync::start_worker(
                app_state.db.clone(),
                app.handle().clone(),
            );
            // 将同一个实例注入到全局状态，避免重复创建导致的不一致
            app.manage(app_state);

            // 初始化全局出站代理 HTTP 客户端
            {
                let db = &app.state::<AppState>().db;
                let proxy_url = db.get_global_proxy_url().ok().flatten();

                if let Err(e) = crate::proxy::http_client::init(proxy_url.as_deref()) {
                    log::error!(
                        "[GlobalProxy] [GP-005] Failed to initialize with saved config: {e}"
                    );

                    // 清除无效的代理配置
                    if proxy_url.is_some() {
                        log::warn!(
                            "[GlobalProxy] [GP-006] Clearing invalid proxy config from database"
                        );
                        if let Err(clear_err) = db.set_global_proxy_url(None) {
                            log::error!(
                                "[GlobalProxy] [GP-007] Failed to clear invalid config: {clear_err}"
                            );
                        }
                    }

                    // 使用直连模式重新初始化
                    if let Err(fallback_err) = crate::proxy::http_client::init(None) {
                        log::error!(
                            "[GlobalProxy] [GP-008] Failed to initialize direct connection: {fallback_err}"
                        );
                    }
                }
            }

            // 异常退出恢复 + 代理状态自动恢复
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = app_handle.state::<AppState>();

                // 必须排在 auto-extract 之前：先把历史泄漏进 Gemini 共享片段的凭据
                // 清干净，否则紧接着的提取会基于被污染的 live 再写一遍。

                // 启动流程走完：托盘这时才开始报「路由服务没在运行」。
                crate::tray::mark_startup_settled(&app_handle);

                // Periodic backup check (on startup)
                if let Err(e) = state.db.periodic_backup_if_needed() {
                    log::warn!("Periodic backup failed on startup: {e}");
                }

                // Periodic maintenance timer: run once per day while the app is running
                let db_for_timer = state.db.clone();
                tauri::async_runtime::spawn(async move {
                    const PERIODIC_MAINTENANCE_INTERVAL_SECS: u64 = 24 * 60 * 60;
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                        PERIODIC_MAINTENANCE_INTERVAL_SECS,
                    ));
                    interval.tick().await; // skip immediate first tick (already checked above)
                    loop {
                        interval.tick().await;
                        if let Err(e) = db_for_timer.periodic_backup_if_needed() {
                            log::warn!("Periodic maintenance timer failed: {e}");
                        }
                    }
                });

                // 启动时补齐既有请求记录的费用（只写数据库，不读任何客户端会话文件）
                let db_for_cost_backfill = state.db.clone();
                tauri::async_runtime::spawn(async move {
                    let task = tauri::async_runtime::spawn_blocking(move || {
                        if let Err(error) = db_for_cost_backfill.backfill_missing_usage_costs() {
                            log::warn!("Usage cost startup backfill failed: {error}");
                        }
                    });
                    if let Err(error) = task.await {
                        log::warn!("Usage cost backfill task failed: {error}");
                    }
                });
            });

            // Linux: 禁用 WebKitGTK 硬件加速，防止 EGL 初始化失败导致白屏
            #[cfg(target_os = "linux")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.with_webview(|webview| {
                        use webkit2gtk::{WebViewExt, SettingsExt, HardwareAccelerationPolicy};
                        let wk_webview = webview.inner();
                        if let Some(settings) = WebViewExt::settings(&wk_webview) {
                            SettingsExt::set_hardware_acceleration_policy(&settings, HardwareAccelerationPolicy::Never);
                            log::info!("已禁用 WebKitGTK 硬件加速");
                        }
                    });
                }
            }

            // 静默启动：根据设置决定是否显示主窗口
            let settings = crate::settings::get_settings();
            if let Some(window) = app.get_webview_window("main") {
                // 在窗口首次显示前同步装饰状态，避免前端加载后再切换导致标题栏闪烁
                // Linux：由设置决定（解决 Wayland 下系统窗口按钮不可用的问题）
                #[cfg(target_os = "linux")]
                let _ = window.set_decorations(!settings.use_app_window_controls);
                // Windows：一律去掉系统标题栏，用页头里的应用内窗口按钮；保留阴影和边缘缩放
                #[cfg(target_os = "windows")]
                {
                    let _ = window.set_decorations(false);
                    let _ = window.set_shadow(true);
                }
                // 上次退出时在轻量模式：照旧只留托盘。由深链接冷启动的要给用户看导入对话框，正常启动。
                if settings.lightweight_mode && !startup_deeplink {
                    if let Err(e) = crate::lightweight::enter_lightweight_mode(app.handle()) {
                        log::error!("启动时恢复轻量模式失败: {e}");
                    } else {
                        log::info!("上次退出时在轻量模式，启动后已恢复");
                    }
                } else if settings.silent_startup {
                    // 静默启动模式：保持窗口隐藏
                    let _ = window.hide();
                    #[cfg(target_os = "windows")]
                    let _ = window.set_skip_taskbar(true);
                    #[cfg(target_os = "macos")]
                    tray::apply_tray_policy(app.handle(), false);
                    log::info!("静默启动模式：主窗口已隐藏");
                } else {
                    // 正常启动模式：显示窗口
                    #[cfg(not(target_os = "windows"))]
                    let _ = window.show();
                    #[cfg(target_os = "windows")]
                    log::info!("正常启动模式：等待主页面加载完成后显示主窗口");
                    #[cfg(not(target_os = "windows"))]
                    log::info!("正常启动模式：主窗口已显示");

                    // Linux: 解决首次启动 UI 无响应问题（Tauri #10746 + wry #637）。
                    // 启动时 webview 未获取焦点 + surface 尺寸协商失败，导致点击无效。
                    // 这里做 set_focus + 伪 resize，等价于无视觉版本的"最大化-还原"。
                    #[cfg(target_os = "linux")]
                    {
                        linux_fix::nudge_main_window(window.clone(), "startup");
                    }
                }
            }


            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_providers,
            commands::get_current_provider,
            commands::add_provider,
            commands::update_provider,
            commands::delete_provider,
            commands::switch_provider,
            commands::get_claude_config_status,
            commands::get_config_status,
            commands::get_claude_code_config_path,
            commands::get_config_dir,
            commands::open_config_folder,
            commands::pick_directory,
            commands::open_external,
            commands::get_init_error,
            commands::get_migration_result,
            commands::get_app_config_path,
            commands::open_app_config_folder,
            commands::reveal_app_log_file,
            commands::get_claude_common_config_snippet,
            commands::set_claude_common_config_snippet,
            commands::get_common_config_snippet,
            commands::set_common_config_snippet,
            commands::get_settings,
            commands::save_settings,
            commands::get_rectifier_config,
            commands::set_rectifier_config,
            commands::get_optimizer_config,
            commands::set_optimizer_config,
            commands::get_log_config,
            commands::set_log_config,
            commands::restart_app,
            commands::install_update_and_restart,
            commands::check_app_update_available,
            commands::check_for_updates,
            commands::open_portable_update_download,
            commands::is_portable_mode,
            commands::copy_text_to_clipboard,
            // usage query
            commands::queryProviderUsage,
            commands::testUsageScript,
            // subscription quota
            commands::get_subscription_quota,
            commands::get_coding_plan_quota,
            commands::get_balance,
            // New MCP via config.json (SSOT)
            // Unified MCP management
            // Prompt management removed
            // Profile management (项目配置方案)
            commands::list_profiles,
            commands::create_profile,
            commands::update_profile,
            commands::delete_profile,
            commands::clear_current_profile,
            commands::apply_profile,
            // Fetch OpenAI-compatible and Anthropic model lists. Response data structure:
            // data[].id, data[]?.owned_by. Special: supports Zhipu OpenAI Responses models[].slug.
            commands::fetch_models_for_config,
            // ours: endpoint speed test + custom endpoint management
            commands::test_api_endpoints,
            commands::get_custom_endpoints,
            commands::add_custom_endpoint,
            commands::remove_custom_endpoint,
            commands::update_endpoint_last_used,
            // app_config_dir override via Store
            commands::get_app_config_dir_override,
            commands::set_app_config_dir_override,
            // provider sort order management
            commands::update_providers_sort_order,
            // theirs: config import/export and dialogs
            commands::export_config_to_file,
            commands::import_config_from_file,
            commands::webdav_test_connection,
            commands::webdav_sync_upload,
            commands::webdav_sync_download,
            commands::webdav_sync_save_settings,
            commands::webdav_sync_clear_settings,
            commands::webdav_sync_fetch_remote_info,
            commands::s3_test_connection,
            commands::s3_sync_upload,
            commands::s3_sync_download,
            commands::s3_sync_save_settings,
            commands::s3_sync_fetch_remote_info,
            commands::save_file_dialog,
            commands::open_file_dialog,
            commands::open_zip_file_dialog,
            commands::create_db_backup,
            commands::list_db_backups,
            commands::list_backup_locations,
            commands::delete_backup_location,
            commands::reveal_backup_location,
            commands::restore_db_backup,
            commands::rename_db_backup,
            commands::delete_db_backup,
            // Deep link import
            commands::parse_deeplink,
            commands::merge_deeplink_config,
            commands::fetch_deeplink_icon,
            commands::import_from_deeplink,
            commands::import_from_deeplink_unified,
            commands::peek_pending_deeplink,
            commands::ack_pending_deeplink,
            update_tray_menu,
            // Environment variable management
            commands::check_env_conflicts,
            commands::delete_env_vars,
            commands::restore_env_backup,
            // Skill management (v3.10.0+ unified)
            // Skill management (legacy API compatibility)
            // Auto launch
            commands::set_auto_launch,
            commands::get_auto_launch_status,
            // Proxy server management
            commands::start_proxy_server,
            commands::stop_proxy_server,
            tray::take_tray_navigation,
            tray_panel::tray_panel_state,
            tray_panel::tray_panel_placement,
            tray_panel::tray_panel_layout,
            tray_panel::tray_panel_hide,
            tray_panel::tray_panel_action,
            tray::tray_app_page_seen,
            commands::get_proxy_status,
            commands::get_proxy_config,
            commands::update_proxy_config,
            // Global & Per-App Config
            commands::get_global_proxy_config,
            commands::update_global_proxy_config,
            commands::get_proxy_client_host,
            commands::set_proxy_client_host,
            commands::get_proxy_config_for_app,
            commands::update_proxy_config_for_app,
            commands::get_pricing_model_source,
            commands::set_pricing_model_source,
            commands::is_proxy_running,
            // Proxy failover commands
            commands::get_provider_health,
            commands::reset_circuit_breaker,
            commands::get_circuit_breaker_config,
            commands::update_circuit_breaker_config,
            commands::get_circuit_breaker_stats,
            // Failover queue management
            commands::get_failover_queue,
            commands::get_available_providers_for_failover,
            commands::add_to_failover_queue,
            commands::remove_from_failover_queue,
            commands::get_auto_failover_enabled,
            commands::set_auto_failover_enabled,
            // Usage statistics
            commands::get_usage_summary,
            commands::get_usage_summary_by_app,
            commands::get_usage_trends,
            commands::get_usage_first_date,
            commands::get_provider_stats,
            commands::get_model_stats,
            commands::get_request_logs,
            commands::get_request_detail,
            commands::get_model_pricing,
            commands::update_model_pricing,
            commands::update_model_pricing_batch,
            commands::delete_model_pricing,
            commands::get_models_dev_sync_config,
            commands::save_models_dev_sync_config,
            commands::record_models_dev_sync_result,
            commands::fetch_models_dev,
            commands::check_provider_limits,
            commands::fetch_exchange_rates,
            // Stream health check
            commands::stream_check_provider,
            // Session and app/tool management removed
            // Provider terminal
            commands::open_provider_terminal,
            // Universal Provider management
            commands::get_universal_providers,
            commands::get_universal_provider,
            commands::upsert_universal_provider,
            commands::delete_universal_provider,
            commands::sync_universal_provider,
            // Custom icon library
            commands::get_custom_icons,
            commands::add_custom_icon,
            commands::rename_custom_icon,
            commands::delete_custom_icon,
            // Global upstream proxy
            commands::get_global_proxy_url,
            commands::set_global_proxy_url,
            commands::test_proxy_url,
            commands::get_upstream_proxy_status,
            commands::scan_local_proxies,
            // Window theme control
            commands::set_window_theme,
            // lightweight mode (for testing or low-resource environments)
            commands::enter_lightweight_mode,
            commands::exit_lightweight_mode,
            commands::is_lightweight_mode,
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(|app_handle, event| {
        // 处理退出请求（所有平台）
        if let RunEvent::ExitRequested { api, code, .. } = &event {
            match classify_exit_request(*code) {
                // code 为 None 表示运行时自动触发（如隐藏窗口的 WebView 被回收导致无存活窗口），
                // 此时应仅阻止退出、保持托盘后台运行。
                ExitRequestAction::StayInTray => {
                    log::info!("运行时触发退出请求（无存活窗口），阻止退出以保持托盘后台运行");
                    api.prevent_exit();
                    return;
                }
                // code 为 RESTART_EXIT_CODE：app.restart() 发起的重启（本应用自己的重启
                // 都走 restart_process，不经过这里，此分支只兜底）。
                // 这条路径上 prevent_exit() 会被 Tauri 忽略，事件循环必定退出，随后由
                // Tauri 在 RunEvent::Exit 后用新二进制 re-exec（macOS 会按更新后的
                // Info.plist 解析可执行名）。
                //
                // 绝不能复用下面的异步清理任务：该任务在 tokio 线程调 save_window_state，
                // 持有 window-state 插件锁的同时向主线程查询窗口几何；而主线程此刻正在
                // 退出事件循环，并在插件自带的 RunEvent::Exit 钩子里等待同一把锁——双方
                // 互等造成进程永久卡死（更新已安装但应用冻结、不再重启，见 #3998）。
                //
                // 重启路径交还 Tauri 默认流程即可：
                //   - 窗口状态：插件 Exit 钩子在主线程保存（同线程读取窗口几何，无死锁）
                //   - 托盘图标：Tauri 内部 cleanup_before_exit 清理，正常走 Drop
                //   - 代理/Live 配置：无需恢复，重启后新实例立即接管并恢复代理状态
                //   - 100ms 落盘等待：重启前的 DB 写入均为命令驱动、此刻已完成，
                //     与所有 Tauri 应用默认重启路径的行为一致，无需额外等待
                ExitRequestAction::DeferToTauriRestart => {
                    log::info!("收到重启请求 (code={code:?})，交由 Tauri 默认重启流程 re-exec");
                    RESTART_REQUESTED.store(true, Ordering::SeqCst);
                    return;
                }
                // 其它 Some(_)：用户主动调用 app.exit() 退出（如托盘菜单"退出"），
                // 此时执行清理后退出。
                ExitRequestAction::CleanupAndExit => {}
            }

            log::info!("收到用户主动退出请求 (code={code:?})，开始清理...");
            api.prevent_exit();

            let app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                save_window_state_before_exit(&app_handle);
                cleanup_before_exit(&app_handle).await;
                // 先于 std::process::exit 显式移除托盘图标。
                // 进程直接退出时 Tauri 运行时不走正常 Drop 流程，
                // 不会向 Windows Shell 发送 NIM_DELETE，导致已退出的进程
                // 注册的图标仍残留在系统托盘（鼠标悬停 Shell 才会重绘发现进程已死）。
                remove_tray_icon_before_exit(&app_handle);
                log::info!("清理完成，退出应用");

                // 短暂等待确保所有 I/O 操作（如数据库写入）刷新到磁盘
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                // 使用 std::process::exit 避免再次触发 ExitRequested
                std::process::exit(0);
            });
            return;
        }

        // macOS ⌘Q、Dock「退出」、注销关机走系统 terminate，不发 ExitRequested、只发
        // RunEvent::Exit，回调一返回进程就结束，只能在这里同步补做退出清理。重启也会走到
        // 这里，照上面 DeferToTauriRestart 的约定交还 Tauri 默认流程，不清理。
        if matches!(event, RunEvent::Exit) {
            if !RESTART_REQUESTED.load(Ordering::SeqCst) {
                cleanup_before_system_exit(app_handle);
            }
            return;
        }

        #[cfg(target_os = "macos")]
        {
            // macOS 在 Dock 图标被点击并重新激活应用时会触发 Reopen 事件，这里手动恢复主窗口。
            // RunEvent::Opened（ccswitch:// 链接）不在这里处理：deep-link 插件的 on_event 已把它
            // 转成 on_open_url 回调，这里再处理一遍会让同一条链接触发两次导入。
            if let RunEvent::Reopen { .. } = event {
                if let Some(window) = app_handle.get_webview_window("main") {
                    #[cfg(target_os = "windows")]
                    {
                        let _ = window.set_skip_taskbar(false);
                    }
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                    tray::apply_tray_policy(app_handle, true);
                } else if crate::lightweight::is_lightweight_mode() {
                    if let Err(e) = crate::lightweight::exit_lightweight_mode(app_handle) {
                        log::error!("退出轻量模式重建窗口失败: {e}");
                    }
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (app_handle, event);
        }
    });
}

// ============================================================
// 应用退出清理
// ============================================================

/// 应用退出前的清理工作
///
/// caswitch 不碰客户端文件，退出时没有要「指回直连」的东西，只把本地代理服务停掉。
pub async fn cleanup_before_exit(app_handle: &tauri::AppHandle) {
    if let Some(state) = app_handle.try_state::<store::AppState>() {
        if state.proxy_service.running_now() == Some(true) {
            let _ = state.proxy_service.stop().await;
        }
        log::info!("退出清理完成：代理已停止");
    }
}

/// 系统终止应用时最多等退出清理这么久：停代理服务自带 5 秒超时，指回直连只是写几个文件。
const SYSTEM_EXIT_CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// 系统直接终止应用时（macOS ⌘Q、Dock「退出」、注销关机）的退出清理。
///
/// 这条路没有 `ExitRequested` 可以 `prevent_exit()` 再异步清理，只能在主线程上等清理做完。
/// 清理放到异步运行时的线程上跑、主线程限时等：万一里面有步骤要等主线程，超时后照常退出，
/// 不会把进程卡住。
fn cleanup_before_system_exit(app_handle: &tauri::AppHandle) {
    log::info!("系统终止应用，开始退出清理...");
    let handle = app_handle.clone();
    let task = tauri::async_runtime::spawn(async move { cleanup_before_exit(&handle).await });
    // timeout 要在运行时里构造（它取当前运行时的计时器），所以包一层 async。
    let finished = tauri::async_runtime::block_on(async move {
        tokio::time::timeout(SYSTEM_EXIT_CLEANUP_TIMEOUT, task).await
    });
    if finished.is_err() {
        log::warn!(
            "退出清理 {} 秒内没做完，直接退出",
            SYSTEM_EXIT_CLEANUP_TIMEOUT.as_secs()
        );
    }
}

/// 主动从系统托盘移除托盘图标。
///
/// `std::process::exit` 会绕过 Tauri 运行时，触发不了 `TrayIcon::drop()`，
/// 也就不会向 Windows Shell 发 `NIM_DELETE`。结果是进程退出后托盘里
/// 仍保留一个死图标的缓存占位（Shell 不会主动重绘，需要鼠标悬停才刷新）。
///
/// 通过 `set_visible(false)` 走 `WM_USER_HIDE_TRAYICON` 消息路径，
/// 触发 tray-icon 内部的 `remove_tray_icon` → `Shell_NotifyIconW(NIM_DELETE)`，
/// 在进程结束前干净地把图标摘掉。其它平台 `set_visible(false)` 也是
/// 正常的隐藏/移除语义，作为跨平台兜底也安全。
pub(crate) fn remove_tray_icon_before_exit(app_handle: &tauri::AppHandle) {
    if let Some(tray) = app_handle.tray_by_id(tray::TRAY_ID) {
        if let Err(e) = tray.set_visible(false) {
            log::warn!("退出时移除托盘图标失败: {e}");
        } else {
            log::info!("已显式从系统托盘移除图标");
        }
    }
}

// ============================================================
// 迁移错误对话框辅助函数
// ============================================================

/// 检测是否为中文环境
fn is_chinese_locale() -> bool {
    std::env::var("LANG")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .map(|lang| lang.starts_with("zh"))
        .unwrap_or(false)
}

/// 显示迁移错误对话框
/// 返回 true 表示用户选择重试，false 表示用户选择退出
fn show_migration_error_dialog(app: &tauri::AppHandle, error: &str) -> bool {
    let title = if is_chinese_locale() {
        "配置迁移失败"
    } else {
        "Migration Failed"
    };

    let message = if is_chinese_locale() {
        format!(
            "从旧版本迁移配置时发生错误：\n\n{error}\n\n\
            您的数据尚未丢失，旧配置文件仍然保留。\n\
            建议回退到旧版本 CC Switch 以保护数据。\n\n\
            点击「重试」重新尝试迁移\n\
            点击「退出」关闭程序（可回退版本后重新打开）"
        )
    } else {
        format!(
            "An error occurred while migrating configuration:\n\n{error}\n\n\
            Your data is NOT lost - the old config file is still preserved.\n\
            Consider rolling back to an older CC Switch version.\n\n\
            Click 'Retry' to attempt migration again\n\
            Click 'Exit' to close the program"
        )
    };

    let retry_text = if is_chinese_locale() {
        "重试"
    } else {
        "Retry"
    };
    let exit_text = if is_chinese_locale() {
        "退出"
    } else {
        "Exit"
    };

    // 使用 blocking_show 同步等待用户响应
    // OkCancelCustom: 第一个按钮（重试）返回 true，第二个按钮（退出）返回 false
    app.dialog()
        .message(&message)
        .title(title)
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom(
            retry_text.to_string(),
            exit_text.to_string(),
        ))
        .blocking_show()
}

/// 显示数据库初始化/Schema 迁移失败对话框
/// 返回 true 表示用户选择重试，false 表示用户选择退出
fn show_database_init_error_dialog(
    app: &tauri::AppHandle,
    db_path: &std::path::Path,
    error: &str,
) -> bool {
    let title = if is_chinese_locale() {
        "数据库初始化失败"
    } else {
        "Database Initialization Failed"
    };

    let message = if is_chinese_locale() {
        format!(
            "初始化数据库或迁移数据库结构时发生错误：\n\n{error}\n\n\
            数据库文件路径：\n{db}\n\n\
            您的数据尚未丢失，应用不会自动删除数据库文件。\n\
            常见原因包括：数据库版本过新、文件损坏、权限不足、磁盘空间不足等。\n\n\
            建议：\n\
            1) 先备份整个配置目录（包含 cc-switch.db）\n\
            2) 如果提示“数据库版本过新”，请升级到更新版本\n\
            3) 如果刚升级出现异常，可回退旧版本导出/备份后再升级\n\n\
            点击「重试」重新尝试初始化\n\
            点击「退出」关闭程序",
            db = db_path.display()
        )
    } else {
        format!(
            "An error occurred while initializing or migrating the database:\n\n{error}\n\n\
            Database file path:\n{db}\n\n\
            Your data is NOT lost - the app will not delete the database automatically.\n\
            Common causes include: newer database version, corrupted file, permission issues, or low disk space.\n\n\
            Suggestions:\n\
            1) Back up the entire config directory (including cc-switch.db)\n\
            2) If you see “database version is newer”, please upgrade CC Switch\n\
            3) If this happened right after upgrading, consider rolling back to export/backup then upgrade again\n\n\
            Click 'Retry' to attempt initialization again\n\
            Click 'Exit' to close the program",
            db = db_path.display()
        )
    };

    let retry_text = if is_chinese_locale() {
        "重试"
    } else {
        "Retry"
    };
    let exit_text = if is_chinese_locale() {
        "退出"
    } else {
        "Exit"
    };

    app.dialog()
        .message(&message)
        .title(title)
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom(
            retry_text.to_string(),
            exit_text.to_string(),
        ))
        .blocking_show()
}

// ============================================================
// 退出请求分类
// ============================================================

/// `RunEvent::ExitRequested` 的三类来源，处理方式必须区分。
///
/// 关键约束：重启请求（`code == RESTART_EXIT_CODE`）上 `prevent_exit()` 会被
/// Tauri 静默忽略（见 `ExitRequestApi::prevent_exit` 文档），事件循环必定继续
/// 退出并触发各插件的 `RunEvent::Exit` 钩子；任何与之并发的自定义清理任务都
/// 可能与插件退出钩子争用同一状态而死锁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitRequestAction {
    /// `code` 为 `None`：运行时自动触发（如隐藏窗口的 WebView 被回收导致无存活
    /// 窗口），阻止退出、保持托盘后台运行。
    StayInTray,
    /// `code` 为 `RESTART_EXIT_CODE`：`app.restart()` 发起的重启（本应用自己的
    /// 重启都走 `restart_process`，这里只兜底），不拦截、不做自定义清理，交还
    /// Tauri 默认 re-exec 流程。
    DeferToTauriRestart,
    /// 其它 `Some(_)`：用户主动退出（托盘「退出」等），执行完整异步清理后结束进程。
    CleanupAndExit,
}

/// 收到过重启请求。重启时 Tauri 也会发 `RunEvent::Exit`，靠它跳过系统终止那条清理。
static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);

fn classify_exit_request(code: Option<i32>) -> ExitRequestAction {
    match code {
        None => ExitRequestAction::StayInTray,
        Some(tauri::RESTART_EXIT_CODE) => ExitRequestAction::DeferToTauriRestart,
        Some(_) => ExitRequestAction::CleanupAndExit,
    }
}

// ============================================================
// 在应用主动退出前显式持久化窗口状态
// ============================================================

fn window_state_flags() -> StateFlags {
    StateFlags::POSITION | StateFlags::SIZE | StateFlags::MAXIMIZED
}

/// 当前应用的退出路径会拦截 `ExitRequested` 并最终直接 `std::process::exit(0)`，
/// 这里需要在真正结束进程前手动落盘，避免 window-state 插件的默认退出钩子被绕过。
pub fn save_window_state_before_exit(app_handle: &tauri::AppHandle) {
    if let Err(err) = app_handle.save_window_state(window_state_flags()) {
        log::error!("退出前保存窗口状态失败: {err}");
    } else {
        log::info!("已在退出前保存窗口状态");
    }
}

/// 主动释放 single-instance 锁。
///
/// macOS single-instance 使用 `/tmp/{identifier}.sock`。我们有若干路径会直接
/// `std::process::exit(0)`，不会触发插件挂在 `RunEvent::Exit` 上的清理钩子。
/// 重启前主动 destroy 可以避免新进程误连旧 listener 后自行退出。
pub fn destroy_single_instance_lock(app_handle: &tauri::AppHandle) {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    tauri_plugin_single_instance::destroy(app_handle);
}

/// 清理托盘图标、释放 single-instance 锁后重启当前应用。
///
/// 直接 spawn 新进程 + `exit(0)`（macOS 经 `open -n`，见 `relaunch_macos_bundle`），不经过事件
/// 循环退出，因此 Tauri 内部的 `cleanup_before_exit` 和各插件的
/// `RunEvent::Exit` 钩子都不会执行。需要的清理由调用方与本函数显式补偿：
/// 窗口状态、代理/Live 恢复（调用方）；托盘图标、single-instance 锁（本函数）。
///
/// 有意不调 `AppHandle::cleanup_before_exit()`：它会在调用线程上 Drop 托盘
/// 图标，而 macOS 的 NSStatusItem 操作要求主线程；`set_visible(false)` 走
/// `run_item_main_thread` 代理，跨线程安全（见 `remove_tray_icon_before_exit`）。
pub fn restart_process(app_handle: &tauri::AppHandle) -> ! {
    remove_tray_icon_before_exit(app_handle);
    destroy_single_instance_lock(app_handle);
    let env = app_handle.env();
    #[cfg(target_os = "macos")]
    relaunch_macos_bundle(&env);
    tauri::process::restart(&env);
}

/// macOS 经 LaunchServices（`open -n`）启动新实例，成功即退出；失败时返回，
/// 由调用方回落到 `tauri::process::restart`。
///
/// `tauri::process::restart` 直接 spawn 可执行文件。macOS 14 起应用激活是协作式的：
/// 新进程的 `activateIgnoringOtherApps` 会被系统拒绝，窗口留在其它应用后面。
/// 由当前前台应用请求 LaunchServices 启动，新实例才能拿到前台。
#[cfg(target_os = "macos")]
fn relaunch_macos_bundle(env: &tauri::Env) {
    let Ok(binary) = tauri::process::current_binary(env) else {
        return;
    };
    // <Name>.app/Contents/MacOS/<binary>
    let Some(bundle) = binary
        .ancestors()
        .nth(3)
        .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
    else {
        return;
    };
    let mut command = std::process::Command::new("/usr/bin/open");
    command.arg("-n").arg(bundle);
    let args: Vec<_> = env.args_os.iter().skip(1).collect();
    if !args.is_empty() {
        command.arg("--args").args(args);
    }
    match command.status() {
        Ok(status) if status.success() => std::process::exit(0),
        Ok(status) => log::warn!("open -n 重启失败（{status}），回落直接启动"),
        Err(err) => log::warn!("open -n 重启失败（{err}），回落直接启动"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_exit_request, error_for_log, redact_url_for_log, redact_url_for_log_with_secrets,
        redact_url_origin_for_log, runtime_log_level_allows, ExitRequestAction,
    };

    #[cfg(unix)]
    #[test]
    fn raise_open_file_limit_lifts_soft_limit_to_target_or_hard_limit() {
        fn current() -> libc::rlimit {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            assert_eq!(
                unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
                0
            );
            limit
        }

        let before = current();
        super::raise_open_file_limit();
        let after = current();
        let expected = super::OPEN_FILE_SOFT_LIMIT_TARGET.min(after.rlim_max);
        assert!(after.rlim_cur >= expected);
        assert!(after.rlim_cur >= before.rlim_cur);
        assert_eq!(after.rlim_max, before.rlim_max);
    }

    #[test]
    fn log_error_drops_toml_source_lines_but_keeps_position() {
        let secret = "sk-review-only-secret";
        let toml_edit_error = format!("experimental_bearer_token = \"{secret}\" !\n")
            .parse::<toml_edit::DocumentMut>()
            .unwrap_err()
            .to_string();
        let toml_error = toml::from_str::<toml::Table>(&format!("token = \"{secret}\" !\n"))
            .unwrap_err()
            .to_string();
        for error in [toml_edit_error, toml_error] {
            assert!(error.contains(secret), "前提：诊断里带源码行");
            let logged = error_for_log(&format!("无法解析：{error} (cannot parse: {error})"));
            assert!(!logged.contains(secret), "{logged}");
            assert!(logged.contains("line 1"), "{logged}");
        }
        // 普通错误原样保留。
        assert_eq!(error_for_log("供应商 a 不存在"), "供应商 a 不存在");
    }

    #[test]
    fn log_url_redaction_strips_credentials_and_query_keeps_path() {
        // userinfo 与整个 query 剥离，path 保留用于诊断 base_url 配错。
        assert_eq!(
            redact_url_for_log(
                "https://user:secret@example.com:8443/v1/models?key=top-secret&alt=sse"
            ),
            "https://example.com:8443/v1/models"
        );
        // scheme-relative 保持形态，userinfo 去掉。
        assert_eq!(
            redact_url_for_log("//user:sk-secret@gw.example.com/v1"),
            "//gw.example.com/v1"
        );
        // 无 scheme 的裸 userinfo。
        assert_eq!(
            redact_url_for_log("user:sk-secret@gw.example.com/v1"),
            "gw.example.com/v1"
        );
        // 无法解析为绝对 URL 时：丢 query，其余原样保留。
        assert_eq!(redact_url_for_log("not-a-url?token=secret"), "not-a-url");
        // 不再对 path 段做“看起来像密钥”的形状猜测，正常路径完整保留。
        assert_eq!(
            redact_url_for_log("https://host.example/v1/models/gemini-2.5-pro"),
            "https://host.example/v1/models/gemini-2.5-pro"
        );
    }

    #[test]
    fn log_url_redaction_replaces_known_secret_values() {
        // 精确匹配已知密钥值：无论它出现在 path 还是别处都被抹掉。
        let secrets = vec!["k-9f3a7c2b1e".to_string()];
        assert_eq!(
            redact_url_for_log_with_secrets("https://gw.example.com/k-9f3a7c2b1e/v1", &secrets),
            "https://gw.example.com/[REDACTED]/v1"
        );
        // 过短(<8)的已知值不参与子串脱敏，避免误伤 /v1/ 之类的正常路径。
        let short_secrets = vec!["api".to_string()];
        assert_eq!(
            redact_url_for_log_with_secrets("https://api.example.com/v1", &short_secrets),
            "https://api.example.com/v1"
        );
    }

    #[test]
    fn log_url_origin_drops_path_for_credential_in_path() {
        // 没有已知密钥可脱敏时，凭据可能整个内嵌在 path，只记 origin。
        assert_eq!(
            redact_url_origin_for_log("https://gw.example.com/k-9f3a7c2b1e/v1"),
            "https://gw.example.com"
        );
        assert_eq!(
            redact_url_origin_for_log("https://user:pass@gw.example.com:8443/secret/v1"),
            "https://gw.example.com:8443"
        );
        assert_eq!(
            redact_url_origin_for_log("//gw.example.com/secret/v1"),
            "//gw.example.com"
        );
    }

    #[test]
    fn runtime_log_filter_honors_dynamic_max_level() {
        assert!(!runtime_log_level_allows(
            log::Level::Error,
            log::LevelFilter::Off
        ));
        assert!(runtime_log_level_allows(
            log::Level::Error,
            log::LevelFilter::Info
        ));
        assert!(runtime_log_level_allows(
            log::Level::Info,
            log::LevelFilter::Info
        ));
        assert!(!runtime_log_level_allows(
            log::Level::Debug,
            log::LevelFilter::Info
        ));
    }

    #[test]
    fn no_code_keeps_app_alive_in_tray() {
        assert_eq!(classify_exit_request(None), ExitRequestAction::StayInTray);
    }

    #[test]
    fn restart_exit_code_defers_to_tauri_default_restart() {
        assert_eq!(
            classify_exit_request(Some(tauri::RESTART_EXIT_CODE)),
            ExitRequestAction::DeferToTauriRestart
        );
    }

    #[test]
    fn user_exit_codes_run_cleanup_then_exit() {
        assert_eq!(
            classify_exit_request(Some(0)),
            ExitRequestAction::CleanupAndExit
        );
        assert_eq!(
            classify_exit_request(Some(1)),
            ExitRequestAction::CleanupAndExit
        );
    }
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use crate::app_config::AppType;
use crate::error::AppError;

/// 自定义端点配置（历史兼容，实际存储在 provider.meta.custom_endpoints）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomEndpoint {
    pub url: String,
    pub added_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
}

fn default_true() -> bool {
    true
}

/// 主页面显示的应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisibleApps {
    #[serde(default = "default_true")]
    pub claude: bool,
}

impl Default for VisibleApps {
    fn default() -> Self {
        Self { claude: true }
    }
}

impl VisibleApps {
    /// Check if the specified app is visible
    pub fn is_visible(&self, app: &AppType) -> bool {
        match app {
            AppType::Claude => self.claude,
        }
    }
}

/// WebDAV 同步状态（持久化同步进度信息）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_remote_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_local_manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_remote_manifest_hash: Option<String>,
}

fn default_remote_root() -> String {
    "cc-switch-sync".to_string()
}
fn default_profile() -> String {
    "default".to_string()
}

/// WebDAV 同步设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDavSyncSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub status: WebDavSyncStatus,
}

impl Default for WebDavSyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_sync: false,
            base_url: String::new(),
            username: String::new(),
            password: String::new(),
            remote_root: default_remote_root(),
            profile: default_profile(),
            status: WebDavSyncStatus::default(),
        }
    }
}

impl WebDavSyncSettings {
    pub fn validate(&self) -> Result<(), crate::error::AppError> {
        if self.base_url.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "webdav.base_url.required",
                "WebDAV 地址不能为空",
                "WebDAV URL is required.",
            ));
        }
        if self.username.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "webdav.username.required",
                "WebDAV 用户名不能为空",
                "WebDAV username is required.",
            ));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.base_url = self.base_url.trim().to_string();
        self.username = self.username.trim().to_string();
        self.remote_root = self.remote_root.trim().to_string();
        self.profile = self.profile.trim().to_string();
        if self.remote_root.is_empty() {
            self.remote_root = default_remote_root();
        }
        if self.profile.is_empty() {
            self.profile = default_profile();
        }
    }

    /// Returns true if all credential fields are blank (no config to persist).
    fn is_empty(&self) -> bool {
        self.base_url.is_empty() && self.username.is_empty() && self.password.is_empty()
    }
}

/// S3 同步设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct S3SyncSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key_id: String,
    #[serde(default)]
    pub secret_access_key: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub status: WebDavSyncStatus,
}

impl Default for S3SyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_sync: false,
            region: String::new(),
            bucket: String::new(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
            endpoint: String::new(),
            remote_root: default_remote_root(),
            profile: default_profile(),
            status: WebDavSyncStatus::default(),
        }
    }
}

impl S3SyncSettings {
    pub fn validate(&self) -> Result<(), crate::error::AppError> {
        if self.bucket.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "s3.bucket.required",
                "S3 存储桶不能为空",
                "S3 bucket is required.",
            ));
        }
        if self.region.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "s3.region.required",
                "S3 区域不能为空",
                "S3 region is required.",
            ));
        }
        if self.access_key_id.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "s3.access_key_id.required",
                "S3 Access Key ID 不能为空",
                "S3 Access Key ID is required.",
            ));
        }
        if self.secret_access_key.trim().is_empty() {
            return Err(crate::error::AppError::localized(
                "s3.secret_access_key.required",
                "S3 Secret Access Key 不能为空",
                "S3 Secret Access Key is required.",
            ));
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.region = self.region.trim().to_string();
        self.bucket = self.bucket.trim().to_string();
        self.access_key_id = self.access_key_id.trim().to_string();
        self.endpoint = self.endpoint.trim().to_string();
        self.remote_root = self.remote_root.trim().to_string();
        self.profile = self.profile.trim().to_string();
        if self.remote_root.is_empty() {
            self.remote_root = default_remote_root();
        }
        if self.profile.is_empty() {
            self.profile = default_profile();
        }
    }

    /// Returns true if all credential fields are blank (no config to persist).
    fn is_empty(&self) -> bool {
        self.bucket.is_empty()
            && self.region.is_empty()
            && self.access_key_id.is_empty()
            && self.secret_access_key.is_empty()
    }
}




/// 用量金额的显示方式。只影响显示：库里的费用一律是美元，按 `exchange_rates`
/// （或自动获取到的 `auto_exchange_rates`）换算后显示；值的合法性（认不认得币种、
/// 汇率是否为正数）由前端兜底。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageDisplaySettings {
    /// 显示币种（"USD" / "CNY" / "JPY" 等，见前端 `DISPLAY_CURRENCIES`），缺省为 USD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// 1 美元折多少目标币种，按币种存：换回 USD 再换回来，填过的汇率还在
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exchange_rates: BTreeMap<String, f64>,
    /// 侧栏「今日」显示哪几项（"cost" / "tokens" / "requests" / "hitRate"，最多两项），
    /// 缺省只显示金额；空列表＝不显示，所以空列表也要写进文件
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_today_items: Option<Vec<String>>,
    /// 请求日志的列显隐：只存用户改过的列（列 id → 是否显示），没写的按各列默认值，
    /// 以后新加的列也按它自己的默认值出现
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub request_log_columns: BTreeMap<String, bool>,
    /// 用量趋势图的样式（"bar" / "area"），缺省为柱形图
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trend_chart_type: Option<String>,
    /// 汇率来源："auto" 自动获取（每日更新）/"manual" 手动填写，缺省按 manual
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_source: Option<String>,
    /// 自动获取到的汇率，按币种存；和手填的 `exchange_rates` 分开，切回手动不丢手填值
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub auto_exchange_rates: BTreeMap<String, f64>,
    /// 上次成功取到汇率的时间（毫秒时间戳）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_fetched_at: Option<i64>,
    /// 上次取到汇率的实际数据源，界面署名用（open.er-api.com / floatrates.com）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_provider: Option<String>,
}

/// 应用设置结构
///
/// 存储设备级别设置，保存在本地 `~/.ca-switch/settings.json`，不随数据库同步。
/// 这确保了云同步场景下多设备可以独立运作。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    // ===== 设备级 UI 设置 =====
    #[serde(default = "default_show_in_tray")]
    pub show_in_tray: bool,
    #[serde(default = "default_minimize_to_tray_on_close")]
    pub minimize_to_tray_on_close: bool,
    #[serde(default)]
    pub use_app_window_controls: bool,
    /// 是否开机自启
    #[serde(default)]
    pub launch_on_startup: bool,
    /// 静默启动（程序启动时不显示主窗口，仅托盘运行）
    #[serde(default)]
    pub silent_startup: bool,
    /// 退出时是否处于轻量模式（主窗口已销毁、只留托盘）；下次启动按它恢复。
    /// 只经 [`set_lightweight_mode`] 写，前端全量保存不透传。
    #[serde(default)]
    pub lightweight_mode: bool,
    /// 是否在主页面启用本地代理功能（默认关闭）
    #[serde(default)]
    pub enable_local_proxy: bool,
    /// User has confirmed the local proxy first-run notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_confirmed: Option<bool>,
    /// User has confirmed the usage query first-run notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_confirmed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_dashboard_refresh_interval_ms: Option<u32>,
    /// 用量金额的显示币种与汇率（只影响显示）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_display: Option<UsageDisplaySettings>,
    /// Whether to show the failover toggle independently on the main page
    #[serde(default)]
    pub enable_failover_toggle: bool,
    /// Whether to show the project profile switcher on the main page header
    #[serde(default)]
    pub show_profile_switcher: bool,
    /// Show the provider search button in the provider page header (on by default;
    /// Cmd/Ctrl+F still opens the search when it is hidden)
    #[serde(default = "default_true")]
    pub show_provider_search: bool,
    /// Check installed CLI tools for new versions at startup (off by default:
    /// many users do not want to chase every release).
    #[serde(default)]
    pub check_tool_updates_on_startup: bool,
    /// 点托盘图标出原生菜单，不出托盘面板（macOS / Windows；Linux 只能出原生菜单，不看这一项）。
    /// 默认关：用托盘面板，想要原生菜单在设置里换回。
    #[serde(default)]
    pub native_tray_menu: bool,
    /// User has confirmed the failover toggle first-run notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failover_confirmed: Option<bool>,
    /// User has confirmed the first-run welcome notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_run_notice_confirmed: Option<bool>,
    /// User has confirmed the one-time "new layout" dialog shown to upgrading users
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_layout_notice_confirmed: Option<bool>,
    /// Highest app version whose "what's new" summary the user has seen on this device
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whats_new_seen_version: Option<String>,
    /// User has confirmed the common config first-run notice
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_config_confirmed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// 按档的额度百分比写「剩余」还是「已用」（`"left"` / `"used"`，缺省为剩余）。卡片、
    /// 授权中心和托盘一起变；余额、Credits、重置次数不受影响（#8024）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_display: Option<String>,

    // ===== 主页面显示的应用 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_apps: Option<VisibleApps>,
    /// 应用页和侧栏的应用顺序（应用 ID）。缺的应用由前端按默认顺序插回，不认识的忽略
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_order: Option<Vec<String>>,

    // ===== 设备级目录覆盖 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_config_dir: Option<String>,

    // ===== 当前供应商 ID（设备级）=====
    /// 当前 API 供应商 ID（本地存储，优先于数据库 is_current）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_provider_claude: Option<String>,

    // ===== 本地路由（设备级）=====
    /// 写给客户端的代理主机（IP 或主机名），空/未设时按监听地址推（`0.0.0.0` → `127.0.0.1`）。
    /// 客户端和代理不在同一个网络栈时用，比如 WSL2 NAT 模式里的会话连不到 Windows 的回环。
    /// 只对这台机器有意义，所以不放在会随云同步覆盖的 proxy_config 表里。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_client_host: Option<String>,

    // ===== WebDAV 同步设置 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_sync: Option<WebDavSyncSettings>,

    // ===== S3 同步设置 =====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3_sync: Option<S3SyncSettings>,

    // ===== WebDAV 备份设置（旧版，保留向后兼容）=====
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webdav_backup: Option<serde_json::Value>,

    // ===== 备份策略设置 =====
    /// Auto-backup interval in hours (default 24, 0 = disabled)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_interval_hours: Option<u32>,
    /// Maximum number of backup files to retain (default 10)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_retain_count: Option<u32>,

    // ===== 终端设置 =====
    /// 首选终端应用（可选，默认使用系统默认终端）
    /// - macOS: "terminal" | "iterm2" | "warp" | "alacritty" | "kitty" | "ghostty" | "otty" | "wezterm" | "kaku"
    /// - Windows: "cmd" | "powershell" | "wt" (Windows Terminal)
    /// - Linux: "gnome-terminal" | "konsole" | "xfce4-terminal" | "alacritty" | "kitty" | "ghostty"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_terminal: Option<String>,

}

fn default_show_in_tray() -> bool {
    true
}

fn default_minimize_to_tray_on_close() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_in_tray: true,
            minimize_to_tray_on_close: true,
            use_app_window_controls: false,
            launch_on_startup: false,
            silent_startup: false,
            lightweight_mode: false,
            enable_local_proxy: false,
            proxy_confirmed: None,
            usage_confirmed: None,
            usage_dashboard_refresh_interval_ms: None,
            usage_display: None,
            enable_failover_toggle: false,
            show_profile_switcher: false,
            show_provider_search: true,
            check_tool_updates_on_startup: false,
            native_tray_menu: false,
            failover_confirmed: None,
            first_run_notice_confirmed: None,
            new_layout_notice_confirmed: None,
            whats_new_seen_version: None,
            common_config_confirmed: None,
            language: None,
            quota_display: None,
            visible_apps: None,
            app_order: None,
            claude_config_dir: None,
            current_provider_claude: None,
            proxy_client_host: None,
            webdav_sync: None,
            s3_sync: None,
            webdav_backup: None,
            backup_interval_hours: None,
            backup_retain_count: None,
            preferred_terminal: None,
        }
    }
}

impl AppSettings {
    /// 额度百分比按「已用」写（默认按剩余）
    pub fn quota_shows_used(&self) -> bool {
        self.quota_display.as_deref() == Some("used")
    }

    fn settings_path() -> Option<PathBuf> {
        // settings.json 保留用于旧版本迁移和无数据库场景
        Some(
            crate::config::get_home_dir()
                .join(".ca-switch")
                .join("settings.json"),
        )
    }

    fn normalize_paths(&mut self) {
        self.claude_config_dir = self
            .claude_config_dir
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        self.language = self
            .language
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| matches!(*s, "en" | "zh" | "zh-TW" | "ja" | "tr"))
            .map(|s| s.to_string());

        self.quota_display = self
            .quota_display
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| matches!(*s, "left" | "used"))
            .map(|s| s.to_string());

        if let Some(sync) = &mut self.webdav_sync {
            sync.normalize();
            if sync.is_empty() {
                self.webdav_sync = None;
            }
        }

        if let Some(s3) = &mut self.s3_sync {
            s3.normalize();
            if s3.is_empty() {
                self.s3_sync = None;
            }
        }
    }

    fn load_from_file() -> Self {
        let Some(path) = Self::settings_path() else {
            return Self::default();
        };
        let loaded = load_settings_from_path(&path);
        if loaded.lossy {
            // 原文件没被完整读进来：先留一份原样的副本，之后的保存才不会把用户的设置冲掉。
            // 这会儿备份不了（比如文件正被别的进程独占），就记下来，留给第一次保存前再备份。
            match backup_settings_file(&path) {
                Ok(dest) => log::warn!("设置文件未能完整读取，原文件已备份到 {}", dest.display()),
                Err(err) => {
                    log::error!(
                        "设置文件未能完整读取，备份原文件也失败，保存前会再试: {}: {err}",
                        path.display()
                    );
                    SETTINGS_BACKUP_PENDING.store(true, Ordering::SeqCst);
                }
            }
        }
        let mut settings = loaded.settings;
        settings.normalize_paths();
        settings
    }
}

/// 读取 settings.json 没能完整读进来、原文件也还没备份成功时置位。置位期间每次保存前
/// 都要先把原文件备份出来，备份不成功就拒绝保存，免得用默认值盖掉用户的设置。
static SETTINGS_BACKUP_PENDING: AtomicBool = AtomicBool::new(false);

/// 读文件的 I/O 错误多是暂时的（Windows 上杀毒软件、同步盘短暂独占文件），重试几次再放弃。
const SETTINGS_READ_ATTEMPTS: u32 = 5;
const SETTINGS_READ_RETRY_DELAY: Duration = Duration::from_millis(200);

struct LoadedSettings {
    settings: AppSettings,
    /// 文件存在，但没有被完整读进来：读不了、不是合法 JSON，或丢弃了部分无效字段。
    lossy: bool,
}

fn load_settings_from_path(path: &Path) -> LoadedSettings {
    let bytes = match read_settings_bytes(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return LoadedSettings {
                settings: AppSettings::default(),
                lossy: false,
            }
        }
        Err(err) => {
            log::error!(
                "读取设置文件失败，本次使用默认设置。路径: {}, 错误: {err}",
                path.display()
            );
            return LoadedSettings {
                settings: AppSettings::default(),
                lossy: true,
            };
        }
    };
    let (text, bad_utf8) = match String::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(err) => (String::from_utf8_lossy(err.as_bytes()).into_owned(), true),
    };
    let (settings, parse_lossy) = parse_settings_lenient(&text);
    LoadedSettings {
        settings,
        lossy: bad_utf8 || parse_lossy,
    }
}

/// 文件不存在时返回 `None`；其它 I/O 错误重试几次仍失败才返回。
fn read_settings_bytes(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let mut attempt = 1;
    loop {
        match fs::read(path) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) if attempt >= SETTINGS_READ_ATTEMPTS => return Err(err),
            Err(err) => {
                log::warn!(
                    "读取设置文件失败（第 {attempt} 次），稍后重试: {}: {err}",
                    path.display()
                );
                std::thread::sleep(SETTINGS_READ_RETRY_DELAY);
                attempt += 1;
            }
        }
    }
}

/// 解析设置；整体解析失败时逐个字段挑出还能用的，只丢无效的那几项。
/// 返回值第二项表示是否有内容被丢弃。
fn parse_settings_lenient(content: &str) -> (AppSettings, bool) {
    // 记事本等编辑器会写 UTF-8 BOM，serde_json 不认它，去掉不算丢内容。
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let err = match serde_json::from_str::<AppSettings>(content) {
        Ok(settings) => return (settings, false),
        Err(err) => err,
    };
    // 旧版在个别 Windows 环境下保存时文件没被截短（#8429），新内容后面残留着旧内容的尾巴；
    // 开头那个完整的对象就是最后一次保存的内容，只取它，不因尾巴把整份设置丢掉。
    let first = serde_json::Deserializer::from_str(content)
        .into_iter::<serde_json::Value>()
        .next();
    let Some(Ok(serde_json::Value::Object(map))) = first else {
        log::warn!("设置文件不是合法的 JSON 对象，本次使用默认设置: {err}");
        return (AppSettings::default(), true);
    };

    // 每个字段都有默认值，单独一个键能反序列化就说明它本身是有效的。
    let mut kept = serde_json::Map::new();
    let mut dropped = Vec::new();
    for (key, value) in map {
        let single =
            serde_json::Value::Object(serde_json::Map::from_iter([(key.clone(), value.clone())]));
        if serde_json::from_value::<AppSettings>(single).is_ok() {
            kept.insert(key, value);
        } else {
            dropped.push(key);
        }
    }
    log::warn!("设置文件有无效字段，已丢弃 {dropped:?}，其余设置照常读取: {err}");
    let settings = serde_json::from_value(serde_json::Value::Object(kept)).unwrap_or_default();
    (settings, true)
}

/// 把原文件原样复制一份到同目录 `settings.json.broken-<时间>.bak`。
fn backup_settings_file(path: &Path) -> std::io::Result<PathBuf> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json".to_string());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest = path.with_file_name(format!("{file_name}.broken-{stamp}.bak"));
    // fs::copy 在 Unix 上连同权限位一起复制，副本和原文件一样是 0600（里面可能有同步密码）。
    fs::copy(path, &dest)?;
    Ok(dest)
}

fn save_settings_file(settings: &AppSettings) -> Result<(), AppError> {
    let Some(path) = AppSettings::settings_path() else {
        return Err(AppError::Config("无法获取用户主目录".to_string()));
    };
    write_settings_to_path(&path, settings, &SETTINGS_BACKUP_PENDING)
}

fn write_settings_to_path(
    path: &Path,
    settings: &AppSettings,
    backup_pending: &AtomicBool,
) -> Result<(), AppError> {
    if backup_pending.load(Ordering::SeqCst) {
        match backup_settings_file(path) {
            Ok(dest) => {
                log::warn!("保存设置前已备份未能读取的原文件到 {}", dest.display());
                backup_pending.store(false, Ordering::SeqCst);
            }
            // 原文件已经不在了，没有要保护的内容。
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                backup_pending.store(false, Ordering::SeqCst);
            }
            Err(err) => {
                return Err(AppError::Config(format!(
                    "设置文件之前没能读取，备份原文件也失败，为免覆盖已取消保存 (The settings file could not be read earlier and backing it up failed; saving was cancelled to avoid overwriting it): {}: {err}",
                    path.display()
                )));
            }
        }
    }

    let mut normalized = settings.clone();
    normalized.normalize_paths();
    let json = serde_json::to_string_pretty(&normalized)
        .map_err(|e| AppError::JsonSerialize { source: e })?;
    // 临时文件 + rename：写到一半进程被杀（比如 Windows 更新安装器退出旧进程），
    // 原文件也还是完整的，不会留下截断的 JSON。
    crate::config::atomic_write_private(path, json.as_bytes())
}

static SETTINGS_STORE: OnceLock<RwLock<AppSettings>> = OnceLock::new();

fn settings_store() -> &'static RwLock<AppSettings> {
    SETTINGS_STORE.get_or_init(|| RwLock::new(AppSettings::load_from_file()))
}

pub(crate) fn resolve_override_path(raw: &str) -> PathBuf {
    let join_home = |home: PathBuf, suffix: &str| {
        suffix
            .split(['/', '\\'])
            .filter(|component| !component.is_empty())
            .fold(home, |path, component| path.join(component))
    };

    if raw == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    } else if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return join_home(home, stripped);
        }
    } else if let Some(stripped) = raw.strip_prefix("~\\") {
        if let Some(home) = dirs::home_dir() {
            return join_home(home, stripped);
        }
    }

    PathBuf::from(raw)
}

pub fn get_settings() -> AppSettings {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .clone()
}

pub fn get_settings_for_frontend() -> AppSettings {
    let mut settings = get_settings();
    if let Some(sync) = &mut settings.webdav_sync {
        sync.password.clear();
    }
    if let Some(s3) = &mut settings.s3_sync {
        s3.secret_access_key.clear();
    }
    settings.webdav_backup = None;
    settings
}

pub fn update_settings(mut new_settings: AppSettings) -> Result<(), AppError> {
    new_settings.normalize_paths();
    save_settings_file(&new_settings)?;

    let mut guard = settings_store().write().unwrap_or_else(|e| {
        log::warn!("设置锁已毒化，使用恢复值: {e}");
        e.into_inner()
    });
    *guard = new_settings;
    Ok(())
}

fn mutate_settings<F>(mutator: F) -> Result<(), AppError>
where
    F: FnOnce(&mut AppSettings),
{
    let mut guard = settings_store().write().unwrap_or_else(|e| {
        log::warn!("设置锁已毒化，使用恢复值: {e}");
        e.into_inner()
    });
    let mut next = guard.clone();
    mutator(&mut next);
    next.normalize_paths();
    save_settings_file(&next)?;
    *guard = next;
    Ok(())
}












/// 记下当前是否处于轻量模式（见 [`AppSettings::lightweight_mode`]），值没变就不写盘。
pub fn set_lightweight_mode(enabled: bool) -> Result<(), AppError> {
    if get_settings().lightweight_mode == enabled {
        return Ok(());
    }
    mutate_settings(|settings| settings.lightweight_mode = enabled)
}

/// 客户端地址（见 [`AppSettings::proxy_client_host`]），去掉首尾空白；没设或为空时是 `None`。
pub fn get_proxy_client_host() -> Option<String> {
    get_settings()
        .proxy_client_host
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
}

pub fn set_proxy_client_host(host: &str) -> Result<(), AppError> {
    let host = host.trim();
    mutate_settings(|settings| {
        settings.proxy_client_host = (!host.is_empty()).then(|| host.to_string());
    })
}

/// 从文件重新加载设置到内存缓存
/// 用于导入配置等场景，确保内存缓存与文件同步
pub fn reload_settings() -> Result<(), AppError> {
    let fresh_settings = AppSettings::load_from_file();
    let mut guard = settings_store().write().unwrap_or_else(|e| {
        log::warn!("设置锁已毒化，使用恢复值: {e}");
        e.into_inner()
    });
    *guard = fresh_settings;
    Ok(())
}

pub fn get_claude_override_dir() -> Option<PathBuf> {
    let settings = settings_store().read().ok()?;
    settings
        .claude_config_dir
        .as_ref()
        .map(|p| resolve_override_path(p))
}













// ===== 当前供应商管理函数 =====

/// 获取指定应用类型的当前供应商 ID（从本地 settings 读取）
///
/// 这是设备级别的设置，不随数据库同步。
/// 如果本地没有设置，调用者应该 fallback 到数据库的 `is_current` 字段。
pub fn get_current_provider(app_type: &AppType) -> Option<String> {
    let settings = settings_store().read().ok()?;
    match app_type {
        AppType::Claude => settings.current_provider_claude.clone(),
    }
}

/// 设置指定应用类型的当前供应商 ID（保存到本地 settings）
///
/// 这是设备级别的设置，不随数据库同步。
/// 传入 `None` 会清除当前供应商设置。
pub fn set_current_provider(app_type: &AppType, id: Option<&str>) -> Result<(), AppError> {
    let id_owned = id.map(|s| s.to_string());
    mutate_settings(|settings| match app_type {
        AppType::Claude => settings.current_provider_claude = id_owned.clone(),
    })
}

/// 获取有效的当前供应商 ID（验证存在性）
///
/// 逻辑：
/// 1. 从本地 settings 读取当前供应商 ID
/// 2. 验证该 ID 在数据库中存在
/// 3. 如果不存在则清理本地 settings，fallback 到数据库的 is_current
///
/// 这确保了返回的 ID 一定是有效的（在数据库中存在）。
/// 多设备云同步场景下，配置导入后本地 ID 可能失效，此函数会自动修复。
pub fn get_effective_current_provider(
    db: &crate::database::Database,
    app_type: &AppType,
) -> Result<Option<String>, AppError> {
    // 1. 从本地 settings 读取
    if let Some(local_id) = get_current_provider(app_type) {
        // 2. 验证该 ID 在数据库中存在
        if db
            .get_provider_by_id(&local_id, app_type.as_str())?
            .is_some()
        {
            // 存在，直接返回
            return Ok(Some(local_id));
        }

        // 3. 不存在，清理本地 settings
        log::warn!(
            "本地 settings 中的供应商 {} ({}) 在数据库中不存在，将清理并 fallback 到数据库",
            local_id,
            app_type.as_str()
        );
        let _ = set_current_provider(app_type, None);
    }

    // Fallback 到数据库的 is_current
    db.get_current_provider(app_type.as_str())
}

// ===== 备份策略管理函数 =====

/// Get the effective auto-backup interval in hours (default 24)
pub fn effective_backup_interval_hours() -> u32 {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .backup_interval_hours
        .unwrap_or(24)
}

/// Get the effective backup retain count (default 10, minimum 1)
pub fn effective_backup_retain_count() -> usize {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .backup_retain_count
        .map(|n| (n as usize).max(1))
        .unwrap_or(10)
}

// ===== 终端设置管理函数 =====

/// 获取首选终端应用
pub fn get_preferred_terminal() -> Option<String> {
    settings_store()
        .read()
        .unwrap_or_else(|e| {
            log::warn!("设置锁已毒化，使用恢复值: {e}");
            e.into_inner()
        })
        .preferred_terminal
        .clone()
}

// ===== WebDAV 同步设置管理函数 =====

/// 获取 WebDAV 同步设置
pub fn get_webdav_sync_settings() -> Option<WebDavSyncSettings> {
    settings_store().read().ok()?.webdav_sync.clone()
}

/// 保存 WebDAV 同步设置
pub fn set_webdav_sync_settings(settings: Option<WebDavSyncSettings>) -> Result<(), AppError> {
    mutate_settings(|current| {
        current.webdav_sync = settings;
    })
}

/// 仅更新 WebDAV 同步状态，避免覆写 credentials/root/profile 等字段
pub fn update_webdav_sync_status(status: WebDavSyncStatus) -> Result<(), AppError> {
    mutate_settings(|current| {
        if let Some(sync) = current.webdav_sync.as_mut() {
            sync.status = status;
        }
    })
}

// ===== S3 同步设置管理函数 =====

pub fn get_s3_sync_settings() -> Option<S3SyncSettings> {
    settings_store().read().ok()?.s3_sync.clone()
}

pub fn set_s3_sync_settings(settings: Option<S3SyncSettings>) -> Result<(), AppError> {
    mutate_settings(|current| {
        current.s3_sync = settings;
    })
}

pub fn update_s3_sync_status(status: WebDavSyncStatus) -> Result<(), AppError> {
    mutate_settings(|current| {
        if let Some(s3) = current.s3_sync.as_mut() {
            s3.status = status;
        }
    })
}

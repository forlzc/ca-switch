#![allow(non_snake_case)]

use crate::app_config::AppType;
use crate::init_status::{InitErrorPayload, SkillsMigrationPayload};
use crate::services::ProviderService;
use indexmap::IndexSet;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tauri::AppHandle;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

#[path = "tool_node_requirement.rs"]
mod tool_node_requirement;
#[path = "tool_uninstall.rs"]
mod tool_uninstall;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// 打开外部链接
/// 外部链接白名单：只允许 `http:` / `https:` / `mailto:`。
/// 不带协议的裸域名（如 `example.com/docs`）按旧行为补 `https://`；
/// `javascript:`、`file:`、`data:` 及其他自定义协议一律拒绝（前端已过滤，这里是第二道）。
fn validate_external_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let parsed = match url::Url::parse(raw) {
        Ok(parsed) => parsed,
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            url::Url::parse(&format!("https://{raw}")).map_err(|_| "链接格式无效".to_string())?
        }
        Err(_) => return Err("链接格式无效".to_string()),
    };
    match parsed.scheme() {
        "http" | "https" if parsed.host_str().is_some_and(|h| !h.is_empty()) => Ok(parsed.into()),
        "mailto" => Ok(parsed.into()),
        "http" | "https" => Err("链接格式无效".to_string()),
        _ => Err("只能打开 http、https 或 mailto 链接".to_string()),
    }
}

#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> Result<bool, String> {
    let url = validate_external_url(&url)?;

    app.opener()
        .open_url(&url, None::<String>)
        .map_err(|e| format!("打开链接失败: {e}"))?;

    Ok(true)
}

#[tauri::command]
pub async fn copy_text_to_clipboard(text: String) -> Result<bool, String> {
    // Use spawn_blocking to avoid blocking the async runtime
    // Clipboard access can block on some platforms and may have thread/loop constraints
    tokio::task::spawn_blocking(move || {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| format!("访问系统剪贴板失败: {e}"))?;
        clipboard
            .set_text(text)
            .map_err(|e| format!("写入系统剪贴板失败: {e}"))?;
        Ok(true)
    })
    .await
    .map_err(|e| format!("剪贴板任务执行失败: {e}"))?
}

/// 检查更新
#[tauri::command]
pub async fn check_for_updates(handle: AppHandle) -> Result<bool, String> {
    handle
        .opener()
        .open_url(
            "https://github.com/forlzc/cc-switch/releases/latest",
            None::<String>,
        )
        .map_err(|e| format!("打开更新页面失败: {e}"))?;

    Ok(true)
}

/// 便携版新版本压缩包的下载地址，命名与 release.yml 的「Prepare Windows Assets」一致：
/// `CC-Switch-v<版本>-Windows[-arm64]-Portable.zip`。版本号不像版本号时返回 None。
fn portable_zip_url(version: &str, arm64: bool) -> Option<String> {
    let version = version.trim().trim_start_matches('v');
    let looks_like_version = version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    if !looks_like_version {
        return None;
    }
    let suffix = if arm64 { "-arm64" } else { "" };
    Some(format!(
        "https://github.com/forlzc/cc-switch/releases/download/v{version}/CC-Switch-v{version}-Windows{suffix}-Portable.zip"
    ))
}

/// 便携版更新：直接下载对应架构的新版压缩包（解压覆盖即可，数据在 ~/.ca-switch 不受影响）。
/// 不做原地替换自身 exe：那样很容易被杀毒软件当成病毒拦截。
#[tauri::command]
pub async fn open_portable_update_download(
    handle: AppHandle,
    version: String,
) -> Result<bool, String> {
    let url = portable_zip_url(&version, cfg!(target_arch = "aarch64"))
        .unwrap_or_else(|| "https://github.com/forlzc/cc-switch/releases/latest".to_string());
    handle
        .opener()
        .open_url(url, None::<String>)
        .map_err(|e| format!("打开更新页面失败: {e}"))?;

    Ok(true)
}

/// 判断是否为便携版（绿色版）运行
#[tauri::command]
pub async fn is_portable_mode() -> Result<bool, String> {
    let exe_path = std::env::current_exe().map_err(|e| format!("获取可执行路径失败: {e}"))?;
    if let Some(dir) = exe_path.parent() {
        Ok(dir.join("portable.ini").is_file())
    } else {
        Ok(false)
    }
}

/// 获取应用启动阶段的初始化错误（若有）。
/// 用于前端在早期主动拉取，避免事件订阅竞态导致的提示缺失。
#[tauri::command]
pub async fn get_init_error() -> Result<Option<InitErrorPayload>, String> {
    Ok(crate::init_status::get_init_error())
}

/// 获取 JSON→SQLite 迁移结果（若有）。
/// 只返回一次 true，之后返回 false，用于前端显示一次性 Toast 通知。
#[tauri::command]
pub async fn get_migration_result() -> Result<bool, String> {
    Ok(crate::init_status::take_migration_success())
}

/// 获取 Skills 自动导入（SSOT）迁移结果（若有）。
/// 只返回一次 Some({count})，之后返回 None，用于前端显示一次性 Toast 通知。
#[tauri::command]
pub async fn get_skills_migration_result() -> Result<Option<SkillsMigrationPayload>, String> {
    Ok(crate::init_status::take_skills_migration_result())
}

#[derive(serde::Serialize)]
pub struct ToolVersion {
    name: String,
    version: Option<String>,
    latest_version: Option<String>, // 新增字段：最新版本
    error: Option<String>,
    /// 已定位到可执行文件、但 `--version` 报错退出（装了却跑不起来，如 Node 版本不达标）。
    /// 供前端区分"未安装"与"已安装·无法运行"，无需匹配 error 文案反推语义。
    installed_but_broken: bool,
    /// 工具运行环境: "windows", "wsl", "macos", "linux", "unknown"
    env_type: String,
    /// 当 env_type 为 "wsl" 时，返回该工具绑定的 WSL distro（用于按 distro 探测 shells）
    wsl_distro: Option<String>,
    /// 有新版本、但新版本要求的 Node 比这处安装用的新：前端不给升级按钮，提示先升级 Node。
    update_blocked_by_node: Option<tool_node_requirement::NodeRequirementGap>,
}

const VALID_TOOLS: [&str; 13] = [
    "claude", "codex", "gemini", "grok", "opencode", "openclaw", "hermes", "pi", "mcode", "kimi",
    "dsh", "agy", "omp",
];

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslShellPreferenceInput {
    #[serde(default)]
    pub wsl_shell: Option<String>,
    #[serde(default)]
    pub wsl_shell_flag: Option<String>,
}

// Keep platform-specific env detection in one place to avoid repeating cfg blocks.
#[cfg(target_os = "windows")]
fn tool_env_type_and_wsl_distro(tool: &str) -> (String, Option<String>) {
    if let Some(distro) = wsl_distro_for_tool(tool) {
        ("wsl".to_string(), Some(distro))
    } else {
        ("windows".to_string(), None)
    }
}

#[cfg(target_os = "macos")]
fn tool_env_type_and_wsl_distro(_tool: &str) -> (String, Option<String>) {
    ("macos".to_string(), None)
}

#[cfg(target_os = "linux")]
fn tool_env_type_and_wsl_distro(_tool: &str) -> (String, Option<String>) {
    ("linux".to_string(), None)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn tool_env_type_and_wsl_distro(_tool: &str) -> (String, Option<String>) {
    ("unknown".to_string(), None)
}

#[tauri::command]
pub async fn get_tool_versions(
    tools: Option<Vec<String>>,
    wsl_shell_by_tool: Option<HashMap<String, WslShellPreferenceInput>>,
) -> Result<Vec<ToolVersion>, String> {
    let requested: Vec<&str> = if let Some(tools) = tools.as_ref() {
        let set: std::collections::HashSet<&str> = tools.iter().map(|s| s.as_str()).collect();
        VALID_TOOLS
            .iter()
            .copied()
            .filter(|t| set.contains(t))
            .collect()
    } else {
        VALID_TOOLS.to_vec()
    };
    let mut results = Vec::new();

    for tool in requested {
        let pref = wsl_shell_by_tool.as_ref().and_then(|m| m.get(tool));
        let tool_wsl_shell = pref.and_then(|p| p.wsl_shell.as_deref());
        let tool_wsl_shell_flag = pref.and_then(|p| p.wsl_shell_flag.as_deref());

        results.push(get_single_tool_version_impl(tool, tool_wsl_shell, tool_wsl_shell_flag).await);
    }

    Ok(results)
}

// 不同工具仍可能共用 pnpm/npm 的全局目录。先保守地串行化所有安装写入，
// 并在排队前锁定工具，防止页面重挂或另一 IPC 调用重复提交同一工具。
struct ToolLifecycleCoordinator {
    tools: HashMap<&'static str, Arc<tokio::sync::Mutex<()>>>,
    execution: Arc<tokio::sync::Mutex<()>>,
}

impl Default for ToolLifecycleCoordinator {
    fn default() -> Self {
        Self {
            tools: VALID_TOOLS
                .iter()
                .map(|&tool| (tool, Arc::new(tokio::sync::Mutex::new(()))))
                .collect(),
            execution: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

impl ToolLifecycleCoordinator {
    async fn run<F>(&self, tools: Vec<&'static str>, operation: F) -> Result<(), String>
    where
        F: FnOnce(&[&str]) -> Result<(), String> + Send + 'static,
    {
        let tool_guards = tools
            .iter()
            .map(|tool| {
                self.tools
                    .get(tool)
                    .ok_or_else(|| format!("Unsupported tool action target: {tool}"))?
                    .clone()
                    .try_lock_owned()
                    // 稳定错误码供前端区分后台任务仍在进行与真正的执行失败。
                    .map_err(|_| "TOOL_ACTION_IN_PROGRESS".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let execution_guard = self.execution.clone().lock_owned().await;

        // 必须把锁移进 blocking 任务：即使等待它的 IPC future 被取消，
        // 子进程仍会继续运行，直到它真正退出前都不能允许下一次写入。
        tokio::task::spawn_blocking(move || {
            let _tool_guards = tool_guards;
            let _execution_guard = execution_guard;
            operation(&tools)
        })
        .await
        .map_err(|e| format!("tool lifecycle task join error: {e}"))?
    }
}

static TOOL_LIFECYCLE: Lazy<ToolLifecycleCoordinator> =
    Lazy::new(ToolLifecycleCoordinator::default);

/// Display-only plan. Execution regenerates it under the lifecycle lock and rejects changes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolUninstallPlan {
    tool: String,
    path: String,
    real_path: String,
    command: String,
    native: bool,
    wsl_distro: Option<String>,
}

/// One installation offered in the uninstall dialog: a plan, or why it must be removed manually.
#[derive(Debug, serde::Serialize)]
pub struct ToolUninstallCandidate {
    #[serde(flatten)]
    install: ToolInstallation,
    plan: Option<ToolUninstallPlan>,
    unavailable_reason: Option<String>,
    /// A copy the tool's own updater recreates (Grok in npm mode), so removing it alone is temporary.
    reinstalled_by_updater: bool,
}

#[tauri::command]
pub async fn prepare_tool_uninstall(
    tool: String,
    wsl_shell_by_tool: Option<HashMap<String, WslShellPreferenceInput>>,
) -> Result<Vec<ToolUninstallCandidate>, String> {
    if !VALID_TOOLS.contains(&tool.as_str()) {
        return Err(format!("Unsupported tool action target: {tool}"));
    }
    tokio::task::spawn_blocking(move || {
        let pref = wsl_shell_by_tool
            .as_ref()
            .and_then(|prefs| prefs.get(&tool));
        tool_uninstall::candidates(&tool, pref)
    })
    .await
    .map_err(|e| format!("uninstall plan task join error: {e}"))?
}

#[tauri::command]
pub async fn run_tool_lifecycle_action(
    tools: Vec<String>,
    action: String,
    wsl_shell_by_tool: Option<HashMap<String, WslShellPreferenceInput>>,
    uninstall_plan: Option<ToolUninstallPlan>,
) -> Result<(), String> {
    let requested = normalize_requested_tools(&tools);
    if requested.is_empty() {
        return Err("No supported tools selected".to_string());
    }

    if action == "uninstall" {
        let plan = uninstall_plan.ok_or("Preview the uninstall plan before confirming.")?;
        if requested.len() != 1 || requested[0] != plan.tool {
            return Err("The uninstall plan does not match the selected tool.".into());
        }
        return TOOL_LIFECYCLE
            .run(requested, move |tools| {
                for tool in tools {
                    let pref = wsl_shell_by_tool
                        .as_ref()
                        .and_then(|prefs| prefs.get(*tool));
                    tool_uninstall::uninstall(tool, pref, &plan)?;
                }
                Ok(())
            })
            .await;
    }
    let action = ToolLifecycleAction::from_str(&action)?;
    if matches!(action, ToolLifecycleAction::Update) {
        for tool in &requested {
            tool_node_requirement::ensure_update_node(tool).await?;
        }
    }

    let label = match action {
        ToolLifecycleAction::Install => "tool_install",
        ToolLifecycleAction::Update => "tool_update",
    };

    // 排队结束后再探测安装目标，与执行一起持锁，避免读到另一升级的中间状态。
    TOOL_LIFECYCLE
        .run(requested, move |tools| {
            let command_line =
                build_tool_lifecycle_command(tools, action, wsl_shell_by_tool.as_ref())?;
            run_tool_lifecycle_silently(&command_line, label)
        })
        .await
}

/// 静默执行工具安装/更新脚本：直接捕获子进程输出并阻塞到命令真正结束，
/// 不再弹出可见终端窗口（与 `launch_terminal_running` 的"开窗即返回"形成对比，
/// 后者仍保留给 provider 切换等需要交互式终端的场景）。
/// 失败时回传 stderr/stdout 末尾若干行，供前端 toast 提示。
#[cfg(not(target_os = "windows"))]
fn run_tool_lifecycle_silently(command_line: &str, _label: &str) -> Result<(), String> {
    use std::process::Command;
    // command_line 是 bash 风格脚本（含 `set -e` 与多行命令）；强制用 bash 执行，
    // 避免用户默认 shell 为 fish/zsh 时 `set -e` 等语义不一致。
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(command_line);
    // GUI App 继承的是 launchd 的窄 PATH，而锚定探测用的是登录 shell —— 见
    // `login_shell_path` doc。命令自身用绝对路径不受影响，但工具**内部** spawn 的
    // npm/node 等子进程、以及 install 链里裸的 npm fallback 都需要真实 PATH。
    if let Some(login_path) = login_shell_path(None) {
        let inherited = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", merge_path_segments(&login_path, &inherited));
    }
    let output = cmd.output().map_err(|e| format!("启动安装进程失败: {e}"))?;
    finish_lifecycle_output(&output)
}

/// Windows 静默执行：command_line 是 .bat 内容（chcp 65001 + @echo off + call/wsl 行，CRLF 分隔），
/// 写临时 .bat 后用 `cmd /C` 执行，`CREATE_NO_WINDOW` 抑制 console 窗口。
#[cfg(target_os = "windows")]
fn run_tool_lifecycle_silently(command_line: &str, label: &str) -> Result<(), String> {
    // 每次调用使用独立目录，避免同进程并发升级时覆盖或删除另一工具的脚本。
    let script_dir = tempfile::Builder::new()
        .prefix("cc_switch_lifecycle_")
        .tempdir()
        .map_err(|e| format!("创建批处理目录失败: {e}"))?;
    let bat_file = script_dir.path().join(format!("{label}.bat"));
    std::fs::write(&bat_file, command_line).map_err(|e| format!("写入批处理文件失败: {e}"))?;

    let output = windows_lifecycle_command(&bat_file, &effective_path_string()).output();

    finish_lifecycle_output(&output.map_err(|e| format!("启动安装进程失败: {e}"))?)
}

#[cfg(target_os = "windows")]
fn windows_lifecycle_command(bat_file: &Path, effective_path: &str) -> std::process::Command {
    let mut command = std::process::Command::new("cmd");
    command
        .arg("/C")
        .arg(bat_file)
        // MSI 自动重启可能丢失用户 PATH（#8171）。执行与探测共用补全后的 PATH，
        // 让裸 npm 兜底及工具内部启动的 node/npm 都能找到；仅设置子进程环境。
        .env("PATH", effective_path)
        .creation_flags(CREATE_NO_WINDOW);
    command
}

/// 把子进程退出结果转成 `Result`：成功返回 `Ok`；失败提取 stderr（空则回退 stdout）
/// 的末尾若干行作为错误详情，避免把整段安装日志塞进 toast。
fn finish_lifecycle_output(output: &std::process::Output) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = decode_command_output(&output.stderr);
    let stdout = decode_command_output(&output.stdout);
    let raw = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let detail = last_lines(raw, 8);
    Err(if detail.is_empty() {
        format!("命令执行失败 (exit code: {:?})", output.status.code())
    } else {
        detail
    })
}

/// 取文本末尾最多 `n` 行（npm / pip 的关键错误通常出现在输出尾部）。
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

pub(crate) fn decode_command_output(bytes: &[u8]) -> String {
    #[cfg(target_os = "windows")]
    {
        decode_windows_command_output(bytes)
    }

    #[cfg(not(target_os = "windows"))]
    {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

#[cfg(target_os = "windows")]
fn decode_windows_command_output(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }

    use windows_sys::Win32::Globalization::{GetACP, GetOEMCP, MultiByteToWideChar};

    fn decode_codepage(bytes: &[u8], codepage: u32) -> Option<String> {
        if codepage == 0 {
            return None;
        }

        let input_len = i32::try_from(bytes.len()).ok()?;
        unsafe {
            let wide_len = MultiByteToWideChar(
                codepage,
                0,
                bytes.as_ptr(),
                input_len,
                std::ptr::null_mut(),
                0,
            );
            if wide_len <= 0 {
                return None;
            }

            let mut wide = vec![0u16; wide_len as usize];
            let written = MultiByteToWideChar(
                codepage,
                0,
                bytes.as_ptr(),
                input_len,
                wide.as_mut_ptr(),
                wide_len,
            );
            if written <= 0 {
                return None;
            }

            Some(String::from_utf16_lossy(&wide[..written as usize]))
        }
    }

    let oem_cp = unsafe { GetOEMCP() };
    if let Some(decoded) = decode_codepage(bytes, oem_cp) {
        return decoded;
    }

    let ansi_cp = unsafe { GetACP() };
    if ansi_cp != oem_cp {
        if let Some(decoded) = decode_codepage(bytes, ansi_cp) {
            return decoded;
        }
    }

    String::from_utf8_lossy(bytes).into_owned()
}

fn normalize_requested_tools(tools: &[String]) -> Vec<&'static str> {
    let set: std::collections::HashSet<&str> = tools.iter().map(|s| s.as_str()).collect();
    VALID_TOOLS
        .iter()
        .copied()
        .filter(|tool| set.contains(tool))
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum ToolLifecycleAction {
    Install,
    Update,
}

impl FromStr for ToolLifecycleAction {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "install" => Ok(Self::Install),
            "update" => Ok(Self::Update),
            _ => Err(format!("Unsupported tool action: {value}")),
        }
    }
}

fn build_tool_lifecycle_command(
    tools: &[&str],
    action: ToolLifecycleAction,
    wsl_shell_by_tool: Option<&HashMap<String, WslShellPreferenceInput>>,
) -> Result<String, String> {
    let mut lines = Vec::new();

    #[cfg(not(target_os = "windows"))]
    {
        // set -e 让任一步失败即中止;set -o pipefail 保留为管道命令的兜底防线。
        // 当前官方 installer 路径已避免 `curl | bash`,但未来若新增管道命令,
        // 仍应让管道前段失败参与整条脚本判定。
        lines.push("set -e".to_string());
        lines.push("set -o pipefail".to_string());
    }

    // .bat 以 UTF-8 写入，cmd 却按控制台代码页（中文系统 936、西文 437/850）解析，
    // 用户名含非 ASCII 字符时锚定的 npm.cmd 路径会乱码（#8034）。首行是纯 ASCII，
    // 任何代码页都能正确解析；chcp 之后的行按 UTF-8 读。CREATE_NO_WINDOW 下这是
    // 独立的隐藏控制台，不影响用户终端。
    #[cfg(target_os = "windows")]
    {
        lines.push("@chcp 65001>nul".to_string());
        lines.push("@echo off".to_string());
    }

    for tool in tools {
        let label = tool_display_name(tool);
        lines.push(format!("echo ========== {label} =========="));

        let pref = wsl_shell_by_tool.and_then(|m| m.get(*tool));
        let line = build_tool_action_line(
            tool,
            action,
            pref.and_then(|p| p.wsl_shell.as_deref()),
            pref.and_then(|p| p.wsl_shell_flag.as_deref()),
        )?;
        lines.push(line);

        #[cfg(target_os = "windows")]
        lines.push("if errorlevel 1 exit /b %errorlevel%".to_string());

        #[cfg(not(target_os = "windows"))]
        lines.push(String::new());
    }

    Ok(lines.join(if cfg!(target_os = "windows") {
        "\r\n"
    } else {
        "\n"
    }))
}

fn tool_display_name(tool: &str) -> &'static str {
    match tool {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "gemini" => "Gemini CLI",
        "grok" => "Grok Build",
        "opencode" => "OpenCode",
        "openclaw" => "OpenClaw",
        "hermes" => "Hermes",
        "pi" => "Pi",
        "omp" => "Oh My Pi",
        "mcode" => "MiniMax Code",
        "kimi" => "Kimi Code",
        "dsh" => "DeepSeek Harness",
        "agy" => "Antigravity CLI",
        _ => "Unknown",
    }
}

/// 官方 shell installer 都不用 `curl | bash` 这种 pipe 形式（仍然用 curl 下载，
/// 只是先落到临时文件再交给 bash 执行）:WSL 分支会在
/// `wsl.exe ... -- sh -c "<cmd>"` 子 shell 里执行命令,外层脚本的 `set -o pipefail`
/// 不会继承进去;而 WSL 默认 shell 可能是 dash/ash,也不能假设支持 `set -o pipefail`。
/// 先下载到 mktemp 文件再交给 bash,能让 curl 失败稳定变成整条命令失败。
const CLAUDE_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://claude.ai/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'";
const OPENCODE_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://opencode.ai/install -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'";
const GROK_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://x.ai/cli/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'";
/// MiniMax Code 官方 installer：用 npm `--prefix` 装进 `~/.minimax-code`（`MCODE_INSTALL_DIR`
/// 可改），系统 Node 不达标时自带隔离 Node；重跑即升级（已是最新则跳过），全程无交互提示。
const MCODE_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://filecdn.minimax.chat/public/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'";
/// Codex 官方独立安装器（POSIX sh 脚本）。独立安装版的 `codex update` 内部跑的正是
/// `curl ... | sh` 且没开 pipefail——实测断网时 curl 失败、sh 读到空脚本 exit 0，
/// `codex update` 仍打印 "Update ran successfully" 并 exit 0。所以不走 `codex update`，
/// 由这里下载到临时文件再执行，curl 失败会如实变成整条命令失败。
/// 只用于锚定升级（Windows 用 `CODEX_INSTALL_WINDOWS_SCRIPT`；WSL 不锚定），故按平台 gate。
#[cfg(not(target_os = "windows"))]
const CODEX_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://chatgpt.com/codex/install.sh -o $tmp && sh $tmp; status=$?; rm -f $tmp; exit $status'";

/// Hermes 官方安装器会自带/选择合适的 Python 运行时。不要再用
/// `python3 -m pip ... || python -m pip ...`:Hermes PyPI 包要求 Python >=3.11,
/// 但 macOS 系统 `python3` 常是 3.9,而 pyenv 下 `python` shim 还可能不存在,会把
/// 真正的 Python 版本问题盖成 "python command exists in these Python versions"。
const HERMES_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'";
const HERMES_UPDATE_UNIX: &str =
    "hermes update || bash -c 'tmp=$(mktemp) && curl -fsSL https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'";

// The npm distribution needs Node >=22.19. The native installer bundles its runtime.
const KIMI_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://code.kimi.com/kimi-code/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'";
#[cfg(target_os = "windows")]
const KIMI_INSTALL_WINDOWS_SCRIPT: &str = "irm https://code.kimi.com/kimi-code/install.ps1 | iex";

// Oh My Pi 的 npm 包只能由 bun 运行（入口是 `#!/usr/bin/env bun` 的 TypeScript）。官方
// installer 有匹配架构的 bun 时走 bun，否则下载并校验预编译二进制到 `~/.local/bin`，
// 所以新装不要求本机先有 bun。
const OMP_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://omp.sh/install -o $tmp && sh $tmp; status=$?; rm -f $tmp; exit $status'";
#[cfg(target_os = "windows")]
const OMP_INSTALL_WINDOWS_SCRIPT: &str = "irm https://omp.sh/install.ps1 | iex";

/// Antigravity CLI 官方 installer：把原生二进制放进 `~/.local/bin`，再跑 `agy install`
/// 把该目录加进 shell 配置。目标处已有 `agy` 时它只提示已安装、exit 0，所以只用于新装；
/// 升级一律走 `agy update`（实测非 TTY 下照常检查并安装）。
const AGY_INSTALL_UNIX: &str =
    "bash -c 'tmp=$(mktemp) && curl -fsSL https://antigravity.google/cli/install.sh -o $tmp && bash $tmp; status=$?; rm -f $tmp; exit $status'";

#[cfg(target_os = "windows")]
const HERMES_INSTALL_WINDOWS_SCRIPT: &str =
    "irm https://raw.githubusercontent.com/NousResearch/hermes-agent/main/scripts/install.ps1 | iex";
#[cfg(target_os = "windows")]
const GROK_INSTALL_WINDOWS_SCRIPT: &str = "irm https://x.ai/cli/install.ps1 | iex";
#[cfg(target_os = "windows")]
const MCODE_INSTALL_WINDOWS_SCRIPT: &str =
    "irm https://filecdn.minimax.chat/public/install.ps1 | iex";
#[cfg(target_os = "windows")]
const AGY_INSTALL_WINDOWS_SCRIPT: &str = "irm https://antigravity.google/cli/install.ps1 | iex";
#[cfg(target_os = "windows")]
const CODEX_INSTALL_WINDOWS_SCRIPT: &str = "irm https://chatgpt.com/codex/install.ps1 | iex";

#[cfg(target_os = "windows")]
fn powershell_encoded_command(script: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    STANDARD.encode(bytes)
}

#[cfg(target_os = "windows")]
fn hermes_install_windows_command() -> String {
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(HERMES_INSTALL_WINDOWS_SCRIPT)
    )
}

#[cfg(target_os = "windows")]
fn grok_install_windows_command() -> String {
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(GROK_INSTALL_WINDOWS_SCRIPT)
    )
}

#[cfg(target_os = "windows")]
fn agy_install_windows_command() -> String {
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(AGY_INSTALL_WINDOWS_SCRIPT)
    )
}

#[cfg(target_os = "windows")]
fn mcode_install_windows_command() -> String {
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(MCODE_INSTALL_WINDOWS_SCRIPT)
    )
}

#[cfg(target_os = "windows")]
fn kimi_install_windows_command() -> String {
    format!(
        "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(KIMI_INSTALL_WINDOWS_SCRIPT)
    )
}

#[cfg(target_os = "windows")]
fn omp_install_windows_command() -> String {
    format!(
        "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(OMP_INSTALL_WINDOWS_SCRIPT)
    )
}

/// 重跑官方 installer 升级「脚本安装」那一处。显式传 `MCODE_INSTALL_DIR`，保证写回
/// 探测到的那个目录，而不是 installer 的默认目录。
#[cfg(not(target_os = "windows"))]
fn mcode_installer_update_command(install_root: &str) -> String {
    format!(
        "MCODE_INSTALL_DIR={} {MCODE_INSTALL_UNIX}",
        shell_single_quote(install_root)
    )
}

#[cfg(target_os = "windows")]
fn mcode_installer_update_command(install_root: &str) -> String {
    // PowerShell 单引号字面量里 `'` 写作 `''`；整段经 EncodedCommand 传入，不经 cmd 解析。
    let script = format!(
        "$env:MCODE_INSTALL_DIR = '{}'; {MCODE_INSTALL_WINDOWS_SCRIPT}",
        install_root.replace('\'', "''")
    );
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(&script)
    )
}

/// 重跑 Kimi Code 官方 installer 升级原生安装那一处。显式传 `KIMI_INSTALL_DIR`，
/// 保证写回探测到的目录（installer 覆盖 `<dir>/bin/kimi`），而不是默认的 `~/.kimi-code`。
#[cfg(not(target_os = "windows"))]
fn kimi_installer_update_command(install_root: &str) -> String {
    format!(
        "KIMI_INSTALL_DIR={} {KIMI_INSTALL_UNIX}",
        shell_single_quote(install_root)
    )
}

#[cfg(target_os = "windows")]
fn kimi_installer_update_command(install_root: &str) -> String {
    // PowerShell 单引号字面量里 `'` 写作 `''`；整段经 EncodedCommand 传入，不经 cmd 解析。
    let script = format!(
        "$env:KIMI_INSTALL_DIR = '{}'; {KIMI_INSTALL_WINDOWS_SCRIPT}",
        install_root.replace('\'', "''")
    );
    format!(
        "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(&script)
    )
}

/// Codex 官方独立安装器（`install.sh` / `install.ps1`）装出的一处安装。
///
/// 布局（2026-09 实测 install.sh、读 install.ps1）：发布包在
/// `<CODEX_HOME>/packages/standalone/releases/<ver>-<target>/`，`current` 指向当前版本；
/// PATH 上的入口放在 `CODEX_INSTALL_DIR`（POSIX 默认 `~/.local/bin` 下的软链，Windows 默认
/// `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin` 这个 junction），真身都解析到发布包里。
/// 它既不归 npm 管，也不在 `prefers_official_update` 里，此前锚定落空、退回裸
/// `npm i -g` 另装一份 npm 版（#7650）。
#[derive(Debug, PartialEq, Eq)]
struct CodexStandaloneInstall {
    /// 入口所在目录，重跑 installer 时作为 `CODEX_INSTALL_DIR` 写回同一处。
    install_dir: String,
    /// 从真身路径截出的 `CODEX_HOME`；GUI 进程拿不到用户 shell 里设的 `CODEX_HOME`，
    /// 不显式传会装进默认的 `~/.codex`，入口指向的那份原地不动。
    codex_home: Option<String>,
}

/// 纯字符串判定、不碰 fs，POSIX / Windows 路径都能在任一平台单测。
fn codex_standalone_install(bin_path: &str, real_target: &str) -> Option<CodexStandaloneInstall> {
    let install_dir = parent_dir(bin_path);
    if install_dir.is_empty() {
        return None;
    }
    let normalized_real = real_target.replace('\\', "/").to_ascii_lowercase();
    if let Some(idx) = normalized_real.find("/packages/standalone/") {
        // `replace` 与 `to_ascii_lowercase` 都不改字节长度，下标可直接用于原串。
        // Windows 的 real 是 canonicalize 出的 `\\?\` verbatim 路径，交给 installer 前还原。
        let home = &real_target[..idx];
        let home = match home.strip_prefix(r"\\?\UNC\") {
            Some(unc) => format!(r"\\{unc}"),
            None => home.strip_prefix(r"\\?\").unwrap_or(home).to_string(),
        };
        if !home.is_empty() {
            return Some(CodexStandaloneInstall {
                install_dir,
                codex_home: Some(home),
            });
        }
    }
    // canonicalize 失败时 real 就是入口本身；Windows 默认入口目录仍能认出来，
    // 只是拿不到 CODEX_HOME，交给 installer 用默认值。
    let normalized_bin = bin_path.replace('\\', "/").to_ascii_lowercase();
    normalized_bin
        .contains("/programs/openai/codex/bin/")
        .then_some(CodexStandaloneInstall {
            install_dir,
            codex_home: None,
        })
}

/// 重跑官方 installer 升级独立安装那一处。显式传 `CODEX_INSTALL_DIR` / `CODEX_HOME`
/// 写回探测到的位置（实测自定义目录时会原地升级、不在默认目录多装一份）；
/// `CODEX_NON_INTERACTIVE=1` 与 `codex update` 自己重跑 installer 时的做法一致。
#[cfg(not(target_os = "windows"))]
fn codex_installer_update_command(install: &CodexStandaloneInstall) -> String {
    let mut env = format!(
        "CODEX_NON_INTERACTIVE=1 CODEX_INSTALL_DIR={}",
        shell_single_quote(&install.install_dir)
    );
    if let Some(home) = &install.codex_home {
        env.push_str(&format!(" CODEX_HOME={}", shell_single_quote(home)));
    }
    format!("{env} {CODEX_INSTALL_UNIX}")
}

#[cfg(target_os = "windows")]
fn codex_installer_update_command(install: &CodexStandaloneInstall) -> String {
    // PowerShell 单引号字面量里 `'` 写作 `''`；整段经 EncodedCommand 传入，不经 cmd 解析。
    let quote = |value: &str| value.replace('\'', "''");
    let mut script = format!(
        "$env:CODEX_NON_INTERACTIVE = '1'; $env:CODEX_INSTALL_DIR = '{}'; ",
        quote(&install.install_dir)
    );
    if let Some(home) = &install.codex_home {
        script.push_str(&format!("$env:CODEX_HOME = '{}'; ", quote(home)));
    }
    script.push_str(CODEX_INSTALL_WINDOWS_SCRIPT);
    format!(
        "powershell -NoProfile -ExecutionPolicy Bypass -EncodedCommand {}",
        powershell_encoded_command(&script)
    )
}

#[cfg(target_os = "windows")]
fn hermes_update_windows_command() -> String {
    // fallback 是 powershell.exe，不是 .cmd/.bat；这里不需要 `call`。PowerShell 的
    // `irm | iex` 已被 EncodedCommand 收进单一参数,避免 `cmd.exe` 解析管道符。
    format!("hermes update || {}", hermes_install_windows_command())
}

#[derive(Debug, Clone, Copy)]
enum LifecycleCommandShell {
    Posix,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    WindowsBatch,
}

fn npm_install_command_for(tool: &str) -> Option<&'static str> {
    match tool {
        "claude" => Some(
            "npm i -g @anthropic-ai/claude-code@latest --ignore-scripts=false --include=optional --allow-scripts=@anthropic-ai/claude-code",
        ),
        "codex" => Some("npm i -g @openai/codex@latest"),
        "gemini" => Some("npm i -g @google/gemini-cli@latest"),
        "grok" => Some(
            "npm i -g @xai-official/grok@latest --ignore-scripts=false --include=optional --allow-scripts=@xai-official/grok",
        ),
        "opencode" => Some(
            "npm i -g opencode-ai@latest --ignore-scripts=false --include=optional --allow-scripts=opencode-ai",
        ),
        "openclaw" => Some("npm i -g openclaw@latest"),
        "pi" => Some("npm i -g @earendil-works/pi-coding-agent@latest"),
        "kimi" => Some("npm i -g @moonshot-ai/kimi-code@latest"),
        "dsh" => Some("npm i -g @deepseek-ai/dsh@latest --engine-strict"),
        "mcode" => Some(
            "npm i -g @minimax-ai/code@latest --ignore-scripts=false --include=optional \"--allow-scripts=@minimax-ai/code,better-sqlite3\"",
        ),
        _ => None,
    }
}

/// `npm i -g` 时须追加的参数（前导空格已含），与 `npm_install_command_for` 的静态命令
/// 保持一致，供锚定到某处 npm 的升级命令复用。
///
/// Claude Code / OpenCode 的 postinstall 把平台 optional dependency 中的原生程序放到
/// bin/claude.exe / bin/opencode.exe。npm 12 拦截该脚本时会留下文本占位文件，
/// Windows 执行后报“与 Windows 版本不兼容”。
/// Grok Build 的 bin/grok 是 node launcher，postinstall 才把 optional dependency 里的
/// 平台二进制解到 `~/.grok/bin/grok-<version>` 并 relink `grok`；被拦后 launcher 只要见到
/// `~/.grok/bin/grok` 就直接运行它、不比对版本，于是 npm 升级 exit 0 却仍跑旧版。
/// MiniMax Code 依赖 better-sqlite3 的安装脚本：npm 12 默认拦截依赖的 install 脚本，
/// 只放行 `--allow-scripts` 列出的包（按注册表包名匹配），被拦后 SQLite 不可用；
/// `--ignore-scripts=false` / `--include=optional` 抵消用户 npmrc 里的相反设置。
/// npm 10/11 上这串参数无副作用（实测 exit 0、脚本照常执行）。整段 `--allow-scripts=…`
/// 加双引号：逗号在 PowerShell 里会被当成数组分隔符，bash/cmd 都会剥掉这层引号。
fn npm_install_extra_args(tool: &str) -> &'static str {
    match tool {
        "dsh" => " --engine-strict",
        "claude" => {
            " --ignore-scripts=false --include=optional --allow-scripts=@anthropic-ai/claude-code"
        }
        "grok" => " --ignore-scripts=false --include=optional --allow-scripts=@xai-official/grok",
        "opencode" => " --ignore-scripts=false --include=optional --allow-scripts=opencode-ai",
        "mcode" => {
            " --ignore-scripts=false --include=optional \"--allow-scripts=@minimax-ai/code,better-sqlite3\""
        }
        _ => "",
    }
}

// `omp update` 自己识别 brew / mise / nix / bun / npm / 预编译二进制并照原渠道升级，
// 非 TTY 下不询问，失败退出码非 0；Nix 安装它只打印提示就 exit 0，见
// `anchored_command_from_paths` 的 Nix 门控。
// MiniMax Code 虽有 `mcode update`，却刻意不列在这里：它在 stdin/stdout 非 TTY 时只打印
// "No interactive confirmation is available" 就 exit 0、并不安装。静默 lifecycle 的
// `cmd.output()` 正是非 TTY，放进来会让 `mcode update || <fallback>` 的兜底永不触发。
fn official_update_args(tool: &str) -> Option<&'static str> {
    match tool {
        "claude" | "codex" | "grok" | "hermes" | "agy" | "omp" => Some("update"),
        "openclaw" => Some("update --yes"),
        "opencode" => Some("upgrade"),
        "kimi" => Some("upgrade --yes"),
        _ => None,
    }
}

fn bare_official_update_command(tool: &str) -> Option<String> {
    official_update_args(tool).map(|args| format!("{tool} {args}"))
}

fn chain_update_commands(
    primary: String,
    fallback: String,
    shell: LifecycleCommandShell,
) -> String {
    if fallback.trim().is_empty() {
        return primary;
    }
    match shell {
        LifecycleCommandShell::Posix => format!("{primary} || {fallback}"),
        // 这段最终会被外层再包成 `call <command>`。fallback 若是 npm.cmd/pnpm.cmd,
        // `||` 右侧也必须显式 `call`,否则批处理会转移控制权并跳过后续工具。
        LifecycleCommandShell::WindowsBatch => format!("{primary} || call {fallback}"),
    }
}

fn tool_action_shell_command_for_shell(
    tool: &str,
    action: ToolLifecycleAction,
    shell: LifecycleCommandShell,
) -> Option<String> {
    if matches!(tool, "kimi" | "omp") {
        let install = match shell {
            LifecycleCommandShell::Posix if tool == "kimi" => KIMI_INSTALL_UNIX.to_string(),
            LifecycleCommandShell::Posix => OMP_INSTALL_UNIX.to_string(),
            #[cfg(target_os = "windows")]
            LifecycleCommandShell::WindowsBatch if tool == "kimi" => kimi_install_windows_command(),
            #[cfg(target_os = "windows")]
            LifecycleCommandShell::WindowsBatch => omp_install_windows_command(),
            #[cfg(not(target_os = "windows"))]
            LifecycleCommandShell::WindowsBatch => return None,
        };
        return Some(match action {
            ToolLifecycleAction::Install => install,
            ToolLifecycleAction::Update => {
                chain_update_commands(bare_official_update_command(tool)?, install, shell)
            }
        });
    }
    // xAI's and MiniMax's primary Windows distribution is the official PowerShell
    // installer. Keep npm as the network/policy fallback, matching the POSIX installer chain.
    #[cfg(target_os = "windows")]
    if matches!(action, ToolLifecycleAction::Install)
        && matches!(shell, LifecycleCommandShell::WindowsBatch)
    {
        let installer = match tool {
            "grok" => Some(grok_install_windows_command()),
            "mcode" => Some(mcode_install_windows_command()),
            _ => None,
        };
        if let Some(installer) = installer {
            return Some(chain_update_commands(
                installer,
                npm_install_command_for(tool)?.to_string(),
                shell,
            ));
        }
    }

    // 没有 npm 包。重跑 installer 不会升级（已安装就直接退出），升级失败也不拿它兜底。
    if tool == "agy" {
        return Some(match (action, shell) {
            (ToolLifecycleAction::Install, LifecycleCommandShell::Posix) => {
                AGY_INSTALL_UNIX.to_string()
            }
            #[cfg(target_os = "windows")]
            (ToolLifecycleAction::Install, LifecycleCommandShell::WindowsBatch) => {
                agy_install_windows_command()
            }
            #[cfg(not(target_os = "windows"))]
            (ToolLifecycleAction::Install, LifecycleCommandShell::WindowsBatch) => return None,
            (ToolLifecycleAction::Update, _) => "agy update".to_string(),
        });
    }

    if tool == "hermes" {
        return Some(
            match (action, shell) {
                (ToolLifecycleAction::Install, LifecycleCommandShell::Posix) => HERMES_INSTALL_UNIX,
                (ToolLifecycleAction::Update, LifecycleCommandShell::Posix) => HERMES_UPDATE_UNIX,
                #[cfg(target_os = "windows")]
                (ToolLifecycleAction::Install, LifecycleCommandShell::WindowsBatch) => {
                    return Some(hermes_install_windows_command());
                }
                #[cfg(target_os = "windows")]
                (ToolLifecycleAction::Update, LifecycleCommandShell::WindowsBatch) => {
                    return Some(hermes_update_windows_command());
                }
                #[cfg(not(target_os = "windows"))]
                (_, LifecycleCommandShell::WindowsBatch) => return None,
            }
            .to_string(),
        );
    }

    let install = npm_install_command_for(tool)?;
    match action {
        ToolLifecycleAction::Install => Some(install.to_string()),
        ToolLifecycleAction::Update => match prefers_official_update(tool, shell)
            .then(|| bare_official_update_command(tool))
            .flatten()
        {
            Some(update) => Some(chain_update_commands(update, install.to_string(), shell)),
            None => Some(install.to_string()),
        },
    }
}

fn tool_action_shell_command(tool: &str, action: ToolLifecycleAction) -> Option<String> {
    #[cfg(target_os = "windows")]
    let shell = LifecycleCommandShell::WindowsBatch;
    #[cfg(not(target_os = "windows"))]
    let shell = LifecycleCommandShell::Posix;

    tool_action_shell_command_for_shell(tool, action, shell)
}

/// Windows host 上的 WSL 分支专用:`tool_action_shell_command` 在 Windows target 编译
/// 出的版本会包含 Windows batch 语义(例如 `|| call npm ...`)且 hermes 会返回
/// Windows PowerShell installer,但跨 `wsl.exe` 边界后跑的是 Linux。这个 wrapper
/// 强制生成 POSIX 版命令。
#[cfg(target_os = "windows")]
fn wsl_tool_action_shell_command(tool: &str, action: ToolLifecycleAction) -> Option<String> {
    match action {
        ToolLifecycleAction::Install => {
            let command = posix_install_command_for(tool);
            if command.is_empty() {
                None
            } else {
                Some(command)
            }
        }
        ToolLifecycleAction::Update => {
            tool_action_shell_command_for_shell(tool, action, LifecycleCommandShell::Posix)
        }
    }
}

fn build_tool_action_line(
    tool: &str,
    action: ToolLifecycleAction,
    wsl_shell: Option<&str>,
    wsl_shell_flag: Option<&str>,
) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        // ① WSL 工具(override 是 UNC `\\wsl$\<distro>\...`):锚定的绝对路径是 Windows
        //    主机路径,跨 wsl.exe 进入 distro 文件系统后无效;且 enumerate 不参与 WSL。
        //    install 走 POSIX 安装优先级,update 走 POSIX 静态/官方 update 命令,
        //    再通过 wsl.exe -d distro -- sh 包一层。
        //    **必须用 wsl_tool_action_shell_command 而非 tool_action_shell_command**:
        //    后者在 Windows target 给 hermes 返回 PowerShell installer,且 Windows batch
        //    语义也不适合跨 wsl.exe;这里统一替换为 POSIX 版安装/更新命令。
        if let Some(distro) = wsl_distro_for_tool(tool) {
            let command = wsl_tool_action_shell_command(tool, action)
                .ok_or_else(|| format!("Unsupported tool action target: {tool}"))?;
            return build_wsl_tool_action_line(&distro, &command, wsl_shell, wsl_shell_flag);
        }
        // ② Windows 原生 update 锚定;install 走静态(install.sh 是 bash 脚本,Windows
        //    无意义)。**`enumerate_tool_installations` 在这里 per-tool 重做、与前端
        //    probe 阶段算过的结果不共享是 by design**:run_tool_lifecycle_action 是
        //    独立 IPC 调用,不信任前端回传的命令字符串(避免命令注入面扩大);前端是
        //    逐工具触发 lifecycle,batch 化会破坏"逐工具独立成败"的 UX。
        let command = match action {
            ToolLifecycleAction::Update => {
                let installs = enumerate_tool_installations(tool);
                match resolve_update_command(tool, &installs) {
                    UpdateCommand::Anchored(command) | UpdateCommand::Static(command) => command,
                    UpdateCommand::Unmanaged => {
                        return Err(unmanaged_update_error(tool, &installs));
                    }
                }
            }
            ToolLifecycleAction::Install => {
                static_fallback_command_for(tool, ToolLifecycleAction::Install)
            }
        };
        if command.is_empty() {
            return Err(format!("Unsupported tool action target: {tool}"));
        }
        // .bat 调用 .cmd/.bat 必须用 `call` 否则当前脚本被替换、后续 `if errorlevel`
        // 行被跳过;对 .exe 加 call 无害(等同直接调用)。锚定命令头部可能是 .cmd
        // (npm/pnpm)或 .exe(volta),静态命令头部是 `npm`(也是 .cmd)、`py` 等——
        // 全部加 `call ` 前缀,风格统一且语义正确。含空格的头部已被 `win_quote_path_for_batch`
        // 加上双引号,call 对带引号的路径解析正常。
        Ok(format!("call {command}"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (wsl_shell, wsl_shell_flag);
        // update 锚定到命令行实际命中的那处（写回同一个 node / brew / 原生安装器），
        // 而非裸 `npm` 落到 PATH 第一个 npm；install 走「上游推荐 || npm 兜底」短路链
        // （有 native installer 的工具如 claude/opencode/hermes），其余仍裸 npm。
        let command = match action {
            ToolLifecycleAction::Update => {
                let installs = enumerate_tool_installations(tool);
                match resolve_update_command(tool, &installs) {
                    UpdateCommand::Anchored(command) | UpdateCommand::Static(command) => command,
                    UpdateCommand::Unmanaged => {
                        return Err(unmanaged_update_error(tool, &installs));
                    }
                }
            }
            ToolLifecycleAction::Install => install_command_for(tool),
        };
        if command.is_empty() {
            return Err(format!("Unsupported tool action target: {tool}"));
        }
        Ok(command)
    }
}

#[cfg(target_os = "windows")]
fn build_wsl_tool_action_line(
    distro: &str,
    command: &str,
    force_shell: Option<&str>,
    force_shell_flag: Option<&str>,
) -> Result<String, String> {
    if !is_valid_wsl_distro_name(distro) {
        return Err(format!("Invalid WSL distro name: {distro}"));
    }

    let shell = force_shell
        .map(|s| s.rsplit('/').next().unwrap_or(s))
        .unwrap_or("sh");
    if !is_valid_shell(shell) {
        return Err(format!("Invalid WSL shell: {shell}"));
    }

    let flag = if let Some(flag) = force_shell_flag {
        if !is_valid_shell_flag(flag) {
            return Err(format!("Invalid WSL shell flag: {flag}"));
        }
        flag
    } else {
        default_flag_for_shell(shell)
    };

    Ok(format!(
        "wsl.exe -d {distro} -- {shell} {flag} {}",
        windows_cmd_double_quote_arg(command)
    ))
}

/// Windows 双引号包裹基础原语:无条件加引号 + 内部 `"` 转义为 `\"`。
/// `windows_cmd_double_quote_arg`(给 wsl.exe 传 bash 命令字符串用)与
/// `win_quote_path_for_batch`(给锚定路径用)都基于它,避免两份 quoter 各自演化、
/// 未来对同一路径产生不一致引用形态。镜像 POSIX 侧 `shell_single_quote` 与
/// `quote_path_if_spaced` 的"重量基础 + 轻量条件包装"两层结构。
#[cfg(target_os = "windows")]
fn win_double_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

#[cfg(target_os = "windows")]
fn windows_cmd_double_quote_arg(value: &str) -> String {
    win_double_quote(value)
}

/// 获取单个工具的版本信息（内部实现）
/// 本机工具的版本（只探测本地，不联网）。
fn probe_local_version(
    tool: &str,
    wsl_distro: Option<&str>,
    wsl_shell: Option<&str>,
    wsl_shell_flag: Option<&str>,
) -> ShellProbe {
    if let Some(distro) = wsl_distro {
        try_get_version_wsl(tool, distro, wsl_shell, wsl_shell_flag)
    } else {
        #[cfg(target_os = "windows")]
        {
            // Probe the PATH-default entry (what `tool` resolves to in a
            // terminal) first, and only fall back to the directory scan when it
            // is genuinely absent (NotFound). Two goals:
            // 1. Keep the displayed "current version" aligned with the version
            //    the user actually runs — a stale shim in a hardcoded fallback
            //    dir (e.g. an old `%APPDATA%\npm`) must not override a newer
            //    PATH install (#4701: "updated but still shows the old version").
            // 2. Mirror the non-Windows structure (`try_get_version` →
            //    `scan_cli_version`).
            // `probe_path_default_version` executes only the real executable
            //    resolved by `where` (App Execution Aliases filtered out), so
            //    it never `cmd /C tool` into a protocol handler.
            match probe_path_default_version(tool) {
                ShellProbe::NotFound(_) => scan_cli_version(tool),
                found => found,
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            // PATH 第一个命令优先；只有它确实没装(NotFound)才去常见目录兜底扫描。
            match try_get_version(tool) {
                ShellProbe::NotFound(_) => scan_cli_version(tool),
                found => found,
            }
        }
    }
}


async fn get_single_tool_version_impl(
    tool: &str,
    wsl_shell: Option<&str>,
    wsl_shell_flag: Option<&str>,
) -> ToolVersion {
    debug_assert!(
        VALID_TOOLS.contains(&tool),
        "unexpected tool name in get_single_tool_version_impl: {tool}"
    );

    // 判断该工具的运行环境 & WSL distro（如有）
    let (env_type, wsl_distro) = tool_env_type_and_wsl_distro(tool);

    // 使用全局 HTTP 客户端（已包含代理配置）
    let client = crate::proxy::http_client::get();

    // 1. 获取本地版本
    let probe = probe_local_version(tool, wsl_distro.as_deref(), wsl_shell, wsl_shell_flag);
    let (local_version, local_error, installed_but_broken) = match probe {
        ShellProbe::Found(v) => (Some(v), None, false),
        ShellProbe::FoundButFailed(e) => (None, Some(e), true),
        // 纯粹没装不带 error：版本列已显示「未安装」，再显示一行英文原因是重复的。
        // WSL 的 shell 参数非法、执行失败等其它原因仍照常上报。
        ShellProbe::NotFound(e) if e.ends_with(NOT_INSTALLED) => (None, None, false),
        ShellProbe::NotFound(e) => (None, Some(e), false),
    };

    // 2. 获取远程最新版本（npm 工具在本地领先 latest 时会按预发布通道补查，见
    //    fetch_npm_latest_for_tool / npm_prerelease_tags）
    let local = local_version.as_deref();
    let latest_version = match tool {
        "claude" => {
            fetch_npm_latest_for_tool(&client, "@anthropic-ai/claude-code", tool, local).await
        }
        "codex" => fetch_npm_latest_for_tool(&client, "@openai/codex", tool, local).await,
        "gemini" => fetch_npm_latest_for_tool(&client, "@google/gemini-cli", tool, local).await,
        "grok" => fetch_npm_latest_for_tool(&client, "@xai-official/grok", tool, local).await,
        "opencode" => {
            if let Some(version) =
                fetch_npm_latest_for_tool(&client, "opencode-ai", tool, local).await
            {
                Some(version)
            } else {
                fetch_github_latest_version(&client, "anomalyco/opencode").await
            }
        }
        "openclaw" => fetch_npm_latest_for_tool(&client, "openclaw", tool, local).await,
        "hermes" => fetch_hermes_latest_version(&client, local).await,
        "pi" => {
            fetch_npm_latest_for_tool(&client, "@earendil-works/pi-coding-agent", tool, local).await
        }
        "mcode" => fetch_npm_latest_for_tool(&client, "@minimax-ai/code", tool, local).await,
        "kimi" => fetch_npm_latest_for_tool(&client, "@moonshot-ai/kimi-code", tool, local).await,
        "dsh" => fetch_npm_latest_for_tool(&client, "@deepseek-ai/dsh", tool, local).await,
        "agy" => fetch_agy_latest_version(&client).await,
        "omp" => fetch_npm_latest_for_tool(&client, "@oh-my-pi/pi-coding-agent", tool, local).await,
        _ => None,
    };

    // 3. 有新版本时，看新版本要求的 Node 与这处安装用的对不对得上
    let update_blocked_by_node = match (local, latest_version.as_deref()) {
        (Some(local), Some(latest))
            if compare_semver(local, latest) == Some(std::cmp::Ordering::Less) =>
        {
            tool_node_requirement::update_node_gap(&client, tool, latest).await
        }
        _ => None,
    };

    ToolVersion {
        name: tool.to_string(),
        version: local_version,
        latest_version,
        error: local_error,
        installed_but_broken,
        env_type,
        wsl_distro,
        update_blocked_by_node,
    }
}

/// 该工具在 npm 上的预发布通道 tag(靠前者优先)。仅当本地版本已**严格领先**
/// `latest` 时才会被补查 —— 让主动在抢先通道的用户(如走 Claude Code 的 `next`)
/// 看到与所在通道对齐的"最新版本",同时绝不把稳定通道用户暴露给预发布版。
/// 返回空切片表示该工具只看 `latest`、不补查。
///
/// 为何不通用覆盖所有工具:各家预发布 tag 命名互不统一(codex=alpha/beta/native、
/// gemini=nightly/preview、openclaw=alpha/beta),且 codex 的 beta/native 是
/// `0.1.x` 时间戳式版本、gemini 有误发的 `false` tag —— 这些脏值虽会被
/// `pick_latest_version` 的版本比较挡掉,但维护成本与误报风险不值当,故暂只为
/// Claude Code 启用。
fn npm_prerelease_tags(tool: &str) -> &'static [&'static str] {
    match tool {
        "claude" => &["next"],
        _ => &[],
    }
}

/// 解析 "2.1.156" / "2.1.156-beta.1" → (主版本三段, 预发布段)。无法解析返回 None。
/// 与前端 `src/lib/version.ts` 的 parseVersion 语义对称(跨语言各实现一份)。
/// patch 用 u64 以容纳 codex 的 `0.1.2505172116` 时间戳式版本而不溢出。
fn parse_semver(v: &str) -> Option<([u64; 3], Vec<String>)> {
    let v = v.trim();
    // `+local` 段命中开发构建形态(git describe `238.g<sha>`,或含 canary /
    // nightly / dev 等开发通道标识,如 Hermes main/canary 自报的
    // `0.22.1+238.g<sha>`)时,版本号可能低于最新 release、内容反而更新,与
    // release 不可比 —— 返回 None,调用方据此保守处理。普通 semver build
    // metadata(如 `1.0.0+linux.1`)按 semver 规范忽略后照常比较。
    if let Some((_, local)) = v.split_once('+') {
        if is_dev_local(local) {
            return None;
        }
    }
    let core_and_pre = v.split('+').next().unwrap_or("");
    let (core, pre) = match core_and_pre.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (core_and_pre, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None; // 多于三段,非法
    }
    let pre_segments = pre
        .map(|p| p.split('.').map(|s| s.to_string()).collect())
        .unwrap_or_default();
    Some(([major, minor, patch], pre_segments))
}

/// local version 段的开发构建形态:git describe(`238.g<sha>`)或常见开发通道标识
/// (canary / nightly / dev / dirty / snapshot)。普通 semver build metadata
/// (如 `linux.1`)不命中 —— 与前端 `src/lib/version.ts` 的 DEV_LOCAL_RE 语义对称。
static DEV_LOCAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\d+\.g[0-9a-f]{4,}|(^|\.)(canary|nightly|dev|dirty|snapshot)(\.|$)")
        .expect("Invalid dev-local regex")
});

fn is_dev_local(local: &str) -> bool {
    DEV_LOCAL_RE.is_match(local)
}

/// 比较两个版本号(遵循 semver:主版本三段优先;core 相等时有预发布 < 无预发布;
/// 预发布段逐段比 —— 数字段按数值、数字段 < 非数字段、非数字段按 ASCII、前缀相同
/// 则段更多者更大)。任一无法解析返回 None,调用方据此保守处理。
fn compare_semver(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    let (ac, ap) = parse_semver(a)?;
    let (bc, bp) = parse_semver(b)?;
    for i in 0..3 {
        match ac[i].cmp(&bc[i]) {
            Ordering::Equal => continue,
            other => return Some(other),
        }
    }
    match (ap.is_empty(), bp.is_empty()) {
        (true, true) => return Some(Ordering::Equal),
        (true, false) => return Some(Ordering::Greater),
        (false, true) => return Some(Ordering::Less),
        (false, false) => {}
    }
    for (x, y) in ap.iter().zip(bp.iter()) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(xv), Ok(yv)) => xv.cmp(&yv),
            (Ok(_), Err(_)) => Ordering::Less, // 数字段 < 非数字段
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.as_str().cmp(y.as_str()),
        };
        if ord != Ordering::Equal {
            return Some(ord);
        }
    }
    Some(ap.len().cmp(&bp.len()))
}

/// 从一次 registry 请求得到的完整 dist-tags 出发,挑选要展示的"最新版本"。
///
/// 规则:默认就是 `latest`;仅当本地版本已**严格领先** `latest`(说明用户主动在
/// 抢先通道)时,才把 `prerelease_tags` 指向的版本纳入比较,取其中能被解析、且
/// 高于 `latest` 的最高者。无法解析或不高于 latest 的脏 tag 一律落选。
fn pick_latest_version(
    dist_tags: &serde_json::Map<String, serde_json::Value>,
    prerelease_tags: &[&str],
    local_version: Option<&str>,
) -> Option<String> {
    use std::cmp::Ordering;
    let latest = dist_tags.get("latest").and_then(|v| v.as_str())?;

    // 本地是否严格领先 latest;任一无法解析则按"未领先"保守处理(只看 latest)。
    let local_ahead = local_version
        .and_then(|local| compare_semver(local, latest))
        .map(|ord| ord == Ordering::Greater)
        .unwrap_or(false);
    if prerelease_tags.is_empty() || !local_ahead {
        return Some(latest.to_string());
    }

    let mut best = latest.to_string();
    for tag in prerelease_tags {
        if let Some(candidate) = dist_tags.get(*tag).and_then(|v| v.as_str()) {
            if compare_semver(candidate, &best) == Some(Ordering::Greater) {
                best = candidate.to_string();
            }
        }
    }
    Some(best)
}

/// npm 包 dist-tags 专用端点的 URL。
///
/// 该端点的响应体就是 dist-tags 对象本身(几十到几千字节);而 `/{package}` 返回的是
/// 含每个历史版本元数据的完整 packument,codex / opencode / openclaw 这类高频发版的包
/// 已有十几到二十几 MB,一次刷新要下几十 MB(#7339)。scoped 包名的 `/` 按 registry
/// 约定转义成 `%2f`。
fn npm_dist_tags_url(package: &str) -> String {
    format!(
        "https://registry.npmjs.org/-/package/{}/dist-tags",
        package.replace('/', "%2f")
    )
}

/// 拉取 npm 包的完整 dist-tags(单次请求即含 latest/next/beta/...)。
///
/// 与 GitHub / PyPI 两条来源一样套 `LATEST_PROBE_TIMEOUT`:取不到就返回 None,由调用方
/// 显示「未知」,而不是沿用全局客户端的 600s 总超时让卡片一直「加载中」。包不存在时
/// 端点返回 404 与一个 JSON 字符串体,解析成 Map 失败,同样落到 None。
async fn fetch_npm_dist_tags(
    client: &reqwest::Client,
    package: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let resp = client
        .get(npm_dist_tags_url(package))
        .timeout(LATEST_PROBE_TIMEOUT)
        .send()
        .await
        .ok()?;
    resp.json::<serde_json::Map<String, serde_json::Value>>()
        .await
        .ok()
}

/// 查询某 npm 工具要展示的"最新版本":取 `latest`,并在本地版本领先时按工具的
/// 预发布通道(见 `npm_prerelease_tags`)补查 —— 复用同一次 registry 响应,无额外请求。
async fn fetch_npm_latest_for_tool(
    client: &reqwest::Client,
    package: &str,
    tool: &str,
    local_version: Option<&str>,
) -> Option<String> {
    let dist_tags = fetch_npm_dist_tags(client, package).await?;
    pick_latest_version(&dist_tags, npm_prerelease_tags(tool), local_version)
}

/// 版本探测请求的请求级超时。全局客户端是给代理转发用的（总超时 600s / 连接 30s），
/// 探测 latest 若沿用它，api.github.com / pypi.org 被阻断或握手后挂起时，Hermes 卡片
/// 与「刷新 / 全部升级」按钮会一直等；探测拿不到就退到下一来源或显示未知即可。
const LATEST_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Helper function to fetch latest version from GitHub releases
/// Antigravity CLI 的 installer 和自升级共用的发布清单：`manifests/<平台>.json` 的 `version`。
async fn fetch_agy_latest_version(client: &reqwest::Client) -> Option<String> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let url = format!(
        "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/{os}_{arch}.json"
    );
    let resp = client
        .get(&url)
        .timeout(LATEST_PROBE_TIMEOUT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let json = resp.json::<serde_json::Value>().await.ok()?;
    json.get("version")
        .and_then(|v| v.as_str())
        .and_then(release_display_version)
}

async fn fetch_github_latest_version(client: &reqwest::Client, repo: &str) -> Option<String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let resp = client
        .get(&url)
        .header("User-Agent", "cc-switch")
        .header("Accept", "application/vnd.github+json")
        .timeout(LATEST_PROBE_TIMEOUT)
        .send()
        .await
        .ok()?;
    let json = resp.json::<serde_json::Value>().await.ok()?;
    github_release_version_from_json(&json)
}

/// 从 GitHub latest release 的 JSON 中提取展示用版本号。
///
/// 优先取 release `name` 里能解析为语义版本的数字，其次才是 `tag_name`（去 `v` 前缀）：
/// Hermes 的 tag 是日历式（`v2026.8.31`），CLI 自报的语义版本 `0.21.0` 只出现在
/// name（"Hermes Agent v0.21.0 (v2026.8.31)"）里。两条路都经 `release_display_version`
/// 过滤——日历式数字若被当成版本号，前端三段解析会成功（2026 > 0）并永久判定
/// "有可用更新"，点升级后版本不变又触发"版本未变"误报；拿不到语义版本宁可返回
/// None，让调用方退到别的来源。限流（403 JSON 只有 message）同样落到 None。
fn github_release_version_from_json(json: &serde_json::Value) -> Option<String> {
    let from_name = json
        .get("name")
        .and_then(|v| v.as_str())
        .and_then(|name| release_display_version(&extract_version(name)));
    from_name.or_else(|| {
        json.get("tag_name")
            .and_then(|v| v.as_str())
            .and_then(|tag| release_display_version(tag.strip_prefix('v').unwrap_or(tag)))
    })
}

/// 只接受能按语义版本解析、且首段不像年份（< 1000）的字符串作展示版本。
fn release_display_version(candidate: &str) -> Option<String> {
    match parse_semver(candidate) {
        Some(([major, ..], _)) if major < 1000 => Some(candidate.to_string()),
        _ => None,
    }
}

/// 兜底来源给出的 latest 若已被本地版本严格超过，则不展示（返回 None）——
/// 展示一个比当前还旧的"最新版本"只会复现用户报告的"最新 < 当前"矛盾。
/// 任一侧无法解析时按"未领先"保守处理，照常展示。
fn drop_latest_behind_local(latest: Option<String>, local_version: Option<&str>) -> Option<String> {
    let latest = latest?;
    let local_leads = local_version
        .and_then(|local| compare_semver(local, &latest))
        .is_some_and(|ord| ord == std::cmp::Ordering::Greater);
    (!local_leads).then_some(latest)
}

/// Hermes 的「最新版本」：GitHub Releases 为主，PyPI 兜底。
///
/// cc-switch 安装/升级 Hermes 走的是官方 install.sh（`git clone` main 分支）与
/// `hermes update`（`git pull`），整条链路与 PyPI 无关；而 PyPI 的 `hermes-agent`
/// 自 0.19.0（2026-07-20）起停更，上游只在 GitHub Releases 发版
/// （#6475 / #6618 / #7033：「最新版本」长期停在 0.19.0、比当前还旧、升级按钮不出现）。
/// 仅当 GitHub 不可达或被限流时才退到 PyPI，且该值已被本地超过时不展示，宁可显示未知。
async fn fetch_hermes_latest_version(
    client: &reqwest::Client,
    local_version: Option<&str>,
) -> Option<String> {
    if let Some(version) = fetch_github_latest_version(client, "NousResearch/hermes-agent").await {
        return Some(version);
    }
    let pypi = fetch_pypi_latest_version(client, "hermes-agent").await;
    drop_latest_behind_local(pypi, local_version)
}

/// Helper function to fetch latest version from PyPI
async fn fetch_pypi_latest_version(client: &reqwest::Client, package: &str) -> Option<String> {
    let url = format!("https://pypi.org/pypi/{package}/json");
    match client.get(&url).timeout(LATEST_PROBE_TIMEOUT).send().await {
        Ok(resp) => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                json.get("info")
                    .and_then(|info| info.get("version"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// 预编译的版本号正则表达式。末尾的 `(\+[\w.-]+)?` 保留 PEP 440 local version
/// 段(git describe 风格,如 Hermes main/canary 开发构建 `0.22.1+238.g<sha>`),
/// 让前端能据此识别开发构建,避免把"版本号更低但内容更新"误报成"有新版本"。
static VERSION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d+\.\d+\.\d+(-[\w.]+)?(\+[\w.-]+)?").expect("Invalid version regex")
});

/// WSL 版本探测载荷先打印的哨兵。用户 shell 的启动文件（Ubuntu 的 update-motd、
/// `/etc/bash.bashrc` 的 sudo 提示、nvm 的 `Using Node vX.Y.Z` 等）输出都在它前面，
/// 解析时只看它之后的内容（#7347）。
#[cfg_attr(not(windows), allow(dead_code))]
const VERSION_PROBE_SENTINEL: &str = "__CCSWITCH_VERSION__";

/// 版本探测交给用户 shell 执行的命令：先打哨兵，再跑 `--version`。
/// 不含单引号，可以直接嵌进外层 `'...'`。
#[cfg_attr(not(windows), allow(dead_code))]
fn version_probe_payload(tool: &str) -> String {
    format!("echo {VERSION_PROBE_SENTINEL}; {tool} --version")
}

/// WSL 未指定 shell 时依次尝试的 flag：交互式初始化（nvm 等写在 rc 里）优先。
/// 卸载探测（`tool_uninstall`）按同一顺序选 flag，否则两边可能命中不同的安装。
#[cfg(any(target_os = "windows", test))]
const WSL_AUTO_SHELL_FLAGS: [&str; 3] = ["-lic", "-lc", "-c"];

/// WSL 未指定 shell 时的版本检测命令（由外层 `sh -c` 执行）：在 `$SHELL` 里按
/// `WSL_AUTO_SHELL_FLAGS` 依次重试，或只用调用方已校验过的 `flag`。
#[cfg(any(target_os = "windows", test))]
fn wsl_auto_shell_command(payload: &str, flag: Option<&str>) -> String {
    let flags = flag.map_or(WSL_AUTO_SHELL_FLAGS.to_vec(), |flag| vec![flag]);
    let last = flags.len() - 1;
    flags
        .iter()
        .enumerate()
        .map(|(i, flag)| {
            // 只有最后一次尝试保留 stderr，供报错展示
            let quiet = if i < last { " 2>/dev/null" } else { "" };
            format!("\"${{SHELL:-sh}}\" {flag} '{payload}'{quiet}")
        })
        .collect::<Vec<_>>()
        .join(" || ")
}

/// 取哨兵**最后一次**出现之后的输出。
///
/// 取最后一次：兜底链 `-lic || -lc || -c` 失败重试时会多次打印哨兵。
/// 按子串而非整行查找：OSC 序列可能没有换行、直接粘在哨兵前面。
/// 找不到哨兵（shell 在执行载荷之前就失败）时原样返回，保持原有行为。
#[cfg_attr(not(windows), allow(dead_code))]
fn after_version_sentinel(output: &str) -> &str {
    match output.rfind(VERSION_PROBE_SENTINEL) {
        Some(i) => output[i + VERSION_PROBE_SENTINEL.len()..].trim(),
        None => output,
    }
}

/// 判断 shell 输出是否表示"找不到该命令"。
/// 只认各 shell 的固定格式，避免把工具自身的 `module not found` 之类错误当成没装：
/// bash/zsh `command not found`，dash/sh `<tool>: not found`，fish `Unknown command`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_shell_command_missing(err: &str, tool: &str) -> bool {
    err.contains("command not found")
        || err.contains(&format!("{tool}: not found"))
        || err.contains("Unknown command")
}

/// 从版本输出中提取纯版本号
fn extract_version(raw: &str) -> String {
    VERSION_RE
        .find(raw)
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| raw.to_string())
}

/// 工具未安装时的统一错误文案；WSL 路径会再拼上 `[WSL:{distro}] ` 前缀。
const NOT_INSTALLED: &str = "not installed or not executable";

/// 已定位到的可执行文件跑 `--version` 却什么都没输出时的诊断。文件在、拿不到版本，
/// 就是「装了但跑不起来」：安装位置列表照样会列出它，这里若报没装，同一行会既在
/// 「已安装」组里、又显示「未安装」和安装按钮。
fn silent_version_probe_detail(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(0) => "`--version` printed nothing".to_string(),
        Some(code) => format!("`--version` exited with code {code} and printed nothing"),
        None => "`--version` was terminated and printed nothing".to_string(),
    }
}

/// 已定位到的可执行文件启动失败（文件损坏、被安全软件拦截、不是有效程序等）时的诊断。
fn launch_failure_detail(tool_path: &Path, err: &std::io::Error) -> String {
    format!("failed to launch {}: {err}", tool_path.display())
}

/// CLI 版本探测的三态结果，跨平台统一各 probe（`try_get_version` /
/// `try_get_version_wsl` / `scan_cli_version`）的返回，进而在 `ToolVersion` 上给出
/// 结构化的 `installed_but_broken` 信号——避免前端靠匹配错误文案反推语义。
///
/// 关键区分"没装"与"装了但 `--version` 自身报错退出"（如工具要求更高的 Node 版本）：
/// 后者必须如实上报、不去别处捞旧版掩盖，否则"升级到新版却跑不起来"会被旧版盖住，
/// 表现为"升级成功但版本号不变"。
enum ShellProbe {
    /// 成功拿到版本号
    Found(String),
    /// 可执行存在、但 `--version` 非零退出（携带诊断信息，如 stderr 末尾若干行）
    FoundButFailed(String),
    /// 没找到该命令（携带描述性消息，供 UI 展示）
    NotFound(String),
}

/// 在非 Windows 平台用用户 shell 执行 `{tool} --version` 探测版本。
///
/// Windows 不走此路径：`cmd /C {tool}` 可能误触发 App Execution Alias /
/// 协议处理器（曾导致 Windows 版整体被禁用），那里改由 `scan_cli_version`
/// 只执行已定位到的真实可执行文件。
#[cfg(not(target_os = "windows"))]
fn try_get_version(tool: &str) -> ShellProbe {
    use std::process::{Command, Stdio};

    let output = {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| is_valid_shell(s))
            .unwrap_or_else(|| "sh".to_string());
        let flag = default_flag_for_shell(&shell);
        let mut cmd = Command::new(shell);
        cmd.arg(flag)
            .arg(format!("{tool} --version"))
            // 与 `resolve_path_default` 相同：交互式 shell（-lic）不能读到继承来的
            // stdin，也不能因持有控制终端被 SIGTTIN 停住（从终端启动时整个应用卡死）。
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        isolate_child_process_group(&mut cmd);
        cmd.spawn().map_err(|e| e.to_string()).and_then(|child| {
            wait_child_output(
                child,
                CommandDeadline::from_timeout(Some(INSTALL_PROBE_TIMEOUT)),
            )
        })
    };

    match output {
        Ok(out) => {
            let stdout = decode_command_output(&out.stdout).trim().to_string();
            let stderr = decode_command_output(&out.stderr).trim().to_string();
            if out.status.success() {
                let raw = if stdout.is_empty() { &stderr } else { &stdout };
                if raw.is_empty() {
                    ShellProbe::NotFound(NOT_INSTALLED.to_string())
                } else {
                    ShellProbe::Found(extract_version(raw))
                }
            } else {
                // exit 127 = shell 找不到命令（可放心 fallback 到搜索路径）；其它非零码
                // = 命令存在但 --version 自身报错退出，须如实上报、不 fallback 掩盖。
                let err = if stderr.is_empty() { stdout } else { stderr };
                if out.status.code() == Some(127) || err.is_empty() {
                    ShellProbe::NotFound(NOT_INSTALLED.to_string())
                } else {
                    ShellProbe::FoundButFailed(last_lines(err.trim(), 4))
                }
            }
        }
        Err(_) => ShellProbe::NotFound(NOT_INSTALLED.to_string()),
    }
}

/// 校验 WSL 发行版名称是否合法
/// WSL 发行版名称只允许字母、数字、连字符、下划线和点（如 `Ubuntu-22.04`）
#[cfg(target_os = "windows")]
fn is_valid_wsl_distro_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Validate that the given shell name is one of the allowed shells.
fn is_valid_shell(shell: &str) -> bool {
    matches!(
        shell.rsplit('/').next().unwrap_or(shell),
        "sh" | "bash" | "zsh" | "fish" | "dash"
    )
}

/// Validate that the given shell flag is one of the allowed flags.
#[cfg(target_os = "windows")]
fn is_valid_shell_flag(flag: &str) -> bool {
    matches!(flag, "-c" | "-lc" | "-lic")
}

/// Return the default invocation flag for the given shell.
fn default_flag_for_shell(shell: &str) -> &'static str {
    match shell.rsplit('/').next().unwrap_or(shell) {
        "dash" | "sh" => "-c",
        "fish" => "-lc",
        _ => "-lic",
    }
}

// 以下 shell 解析辅助函数仅被 macOS/Linux 的终端启动逻辑使用；Windows 非 test 编译下为死代码。
#[cfg_attr(windows, allow(dead_code))]
fn fallback_user_shell() -> &'static str {
    if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/bash"
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn valid_user_shell_path(shell: &str) -> bool {
    if shell.is_empty()
        || !shell.starts_with('/')
        || !is_valid_shell(shell)
        || shell.chars().any(char::is_control)
    {
        return false;
    }

    let path = std::path::Path::new(shell);
    path.is_file() && is_executable_file(path)
}

#[cfg(unix)]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
#[cfg_attr(windows, allow(dead_code))]
fn is_executable_file(path: &std::path::Path) -> bool {
    path.is_file()
}

/// 获取用户默认 shell 的完整路径；异常或被污染的 SHELL 回退到平台默认值。
#[cfg_attr(windows, allow(dead_code))]
fn get_user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| valid_user_shell_path(shell))
        .unwrap_or_else(|| fallback_user_shell().to_string())
}

/// 构建 exec 行：引号保护 shell 路径，交还用户 shell 让其按默认规则加载 rc 配置。
#[cfg_attr(windows, allow(dead_code))]
fn build_exec_line(shell: &str, cwd: Option<&Path>) -> String {
    let quoted_shell = shell_single_quote(shell);

    match shell.rsplit('/').next().unwrap_or(shell) {
        "zsh" => cwd
            .map(|dir| {
                let command = format!(
                    "cd {} || exit 1; exec {} -i",
                    shell_single_quote(&dir.to_string_lossy()),
                    quoted_shell
                );
                format!("exec {} -lc {}", quoted_shell, shell_single_quote(&command))
            })
            .unwrap_or_else(|| format!("exec {quoted_shell} -l")),
        _ => format!("exec {quoted_shell}"),
    }
}

/// 构建 provider 命令行：通过用户 shell 的交互模式执行，确保 GUI 启动的终端也加载用户 PATH。
#[cfg_attr(windows, allow(dead_code))]
fn build_provider_command_line(shell: &str, launch_command: &str, cwd: Option<&Path>) -> String {
    let command = cwd
        .map(|dir| {
            format!(
                "cd {} && {}",
                user_shell_quote(shell, &dir.to_string_lossy()),
                launch_command
            )
        })
        .unwrap_or_else(|| launch_command.to_string());

    format!(
        "{} {} {}",
        shell_single_quote(shell),
        provider_command_flag_for_shell(shell),
        shell_single_quote(&command)
    )
}

#[cfg_attr(windows, allow(dead_code))]
fn provider_command_flag_for_shell(shell: &str) -> &'static str {
    match shell.rsplit('/').next().unwrap_or(shell) {
        "dash" | "sh" => "-c",
        "zsh" => "-lic",
        _ => "-ic",
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn build_final_shell_cd_command(shell: &str, cwd: Option<&Path>) -> String {
    if matches!(shell.rsplit('/').next().unwrap_or(shell), "zsh") {
        return String::new();
    }

    cwd.map(|dir| {
        format!(
            "cd {} || exit 1\n",
            shell_single_quote(&dir.to_string_lossy())
        )
    })
    .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn try_get_version_wsl(
    tool: &str,
    distro: &str,
    force_shell: Option<&str>,
    force_shell_flag: Option<&str>,
) -> ShellProbe {
    use std::process::Command;

    // 防御性断言：tool 只能是预定义的值
    debug_assert!(VALID_TOOLS.contains(&tool), "unexpected tool name: {tool}");

    // 校验 distro 名称，防止命令注入
    if !is_valid_wsl_distro_name(distro) {
        return ShellProbe::NotFound(format!("[WSL:{distro}] invalid distro name"));
    }

    // 构建 Shell 脚本检测逻辑
    let (shell, flag, cmd) = if let Some(shell) = force_shell {
        // Defensive validation: never allow an arbitrary executable name here.
        if !is_valid_shell(shell) {
            return ShellProbe::NotFound(format!("[WSL:{distro}] invalid shell: {shell}"));
        }
        let shell = shell.rsplit('/').next().unwrap_or(shell);
        let flag = if let Some(flag) = force_shell_flag {
            if !is_valid_shell_flag(flag) {
                return ShellProbe::NotFound(format!("[WSL:{distro}] invalid shell flag: {flag}"));
            }
            flag
        } else {
            default_flag_for_shell(shell)
        };

        (shell.to_string(), flag, version_probe_payload(tool))
    } else {
        if let Some(flag) = force_shell_flag {
            if !is_valid_shell_flag(flag) {
                return ShellProbe::NotFound(format!("[WSL:{distro}] invalid shell flag: {flag}"));
            }
        }
        let cmd = wsl_auto_shell_command(&version_probe_payload(tool), force_shell_flag);

        ("sh".to_string(), "-c", cmd)
    };

    let output = Command::new("wsl.exe")
        .args(["-d", distro, "--", &shell, flag, &cmd])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match output {
        Ok(out) => {
            let stdout = decode_command_output(&out.stdout).trim().to_string();
            let stderr = decode_command_output(&out.stderr).trim().to_string();
            // 启动文件的输出都在哨兵之前，只看它之后（#7347）
            let payload_out = after_version_sentinel(&stdout).to_string();
            if out.status.success() {
                let raw = if payload_out.is_empty() {
                    &stderr
                } else {
                    &payload_out
                };
                if raw.is_empty() {
                    ShellProbe::NotFound(format!("[WSL:{distro}] {NOT_INSTALLED}"))
                } else {
                    ShellProbe::Found(extract_version(raw))
                }
            } else {
                let err = if stderr.is_empty() {
                    payload_out
                } else {
                    stderr
                };
                // wsl.exe 透传的退出码不总可靠，故同时用 exit 127 与 shell 的"找不到命令"
                // 文本兜底判别"没装"；其余非零退出视作"装了但 --version 报错"。
                let not_found = err.is_empty()
                    || out.status.code() == Some(127)
                    || is_shell_command_missing(&err, tool);
                if not_found {
                    ShellProbe::NotFound(format!("[WSL:{distro}] {NOT_INSTALLED}"))
                } else {
                    ShellProbe::FoundButFailed(format!(
                        "[WSL:{distro}] {}",
                        last_lines(err.trim(), 4)
                    ))
                }
            }
        }
        Err(e) => ShellProbe::NotFound(format!("[WSL:{distro}] exec failed: {e}")),
    }
}

/// 非 Windows 平台的 WSL 版本检测存根
/// 注意：此函数实际上不会被调用，因为 `wsl_distro_from_path` 在非 Windows 平台总是返回 None。
/// 保留此函数是为了保持 API 一致性，防止未来重构时遗漏。
#[cfg(not(target_os = "windows"))]
fn try_get_version_wsl(
    _tool: &str,
    _distro: &str,
    _force_shell: Option<&str>,
    _force_shell_flag: Option<&str>,
) -> ShellProbe {
    ShellProbe::NotFound("WSL check not supported on this platform".to_string())
}

fn push_unique_path(paths: &mut IndexSet<std::path::PathBuf>, path: std::path::PathBuf) {
    if path.as_os_str().is_empty() {
        return;
    }

    paths.insert(path);
}

fn push_env_single_dir(
    paths: &mut IndexSet<std::path::PathBuf>,
    value: Option<std::ffi::OsString>,
) {
    if let Some(raw) = value {
        push_unique_path(paths, std::path::PathBuf::from(raw));
    }
}

fn extend_from_path_list(
    paths: &mut IndexSet<std::path::PathBuf>,
    value: Option<std::ffi::OsString>,
    suffix: Option<&str>,
) {
    if let Some(raw) = value {
        for p in std::env::split_paths(&raw) {
            let dir = match suffix {
                Some(s) => p.join(s),
                None => p,
            };
            push_unique_path(paths, dir);
        }
    }
}

fn extend_from_cli_path_env(
    paths: &mut IndexSet<std::path::PathBuf>,
    value: Option<std::ffi::OsString>,
) {
    if let Some(raw) = value {
        for p in std::env::split_paths(&raw) {
            if should_skip_cli_path_env_dir(&p) {
                continue;
            }
            push_unique_path(paths, p);
        }
    }
}

fn should_skip_cli_path_env_dir(path: &Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        is_windows_app_execution_alias_dir(path)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = path;
        false
    }
}

#[cfg(target_os = "windows")]
fn is_windows_app_execution_alias_dir(path: &Path) -> bool {
    let normalized = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    normalized
        .trim_end_matches('\\')
        .ends_with("\\microsoft\\windowsapps")
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn push_env_child_dir(
    paths: &mut IndexSet<std::path::PathBuf>,
    value: Option<std::ffi::OsString>,
    child: &str,
) {
    if let Some(raw) = value {
        push_unique_path(paths, std::path::PathBuf::from(raw).join(child));
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn extend_existing_child_search_paths(
    paths: &mut IndexSet<std::path::PathBuf>,
    base: &Path,
    suffix: Option<&str>,
) {
    if !base.exists() {
        return;
    }

    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = match suffix {
                Some(suffix) => entry.path().join(suffix),
                None => entry.path(),
            };
            if path.exists() {
                push_unique_path(paths, path);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn extend_windows_cli_manager_search_paths(paths: &mut IndexSet<std::path::PathBuf>, home: &Path) {
    push_env_single_dir(paths, std::env::var_os("PNPM_HOME"));
    push_env_child_dir(paths, std::env::var_os("VOLTA_HOME"), "bin");
    push_env_single_dir(paths, std::env::var_os("NVM_SYMLINK"));
    push_env_child_dir(paths, std::env::var_os("SCOOP"), "shims");
    push_env_child_dir(paths, std::env::var_os("SCOOP_GLOBAL"), "shims");

    if let Some(nvm_home) = std::env::var_os("NVM_HOME") {
        let nvm_home = std::path::PathBuf::from(nvm_home);
        push_unique_path(paths, nvm_home.clone());
        extend_existing_child_search_paths(paths, &nvm_home, None);
    }

    if let Some(appdata) = dirs::data_dir() {
        let nvm_home = appdata.join("nvm");
        push_unique_path(paths, nvm_home.clone());
        extend_existing_child_search_paths(paths, &nvm_home, None);
    }

    if !home.as_os_str().is_empty() {
        push_unique_path(paths, home.join("scoop").join("shims"));
    }

    if let Some(local_data) = dirs::data_local_dir() {
        push_unique_path(paths, local_data.join("pnpm"));
        push_unique_path(paths, local_data.join("Volta").join("bin"));
        push_unique_path(paths, local_data.join("Yarn").join("bin"));
    }

    let program_data = std::env::var_os("ProgramData")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("C:\\ProgramData"));
    push_unique_path(paths, program_data.join("scoop").join("shims"));
}

/// OpenCode install.sh 路径优先级（见 https://github.com/anomalyco/opencode README）:
///   $OPENCODE_INSTALL_DIR > $XDG_BIN_DIR > $HOME/bin > $HOME/.opencode/bin
/// 额外扫描 Bun 默认全局安装路径（~/.bun/bin）
/// 和 Go 安装路径（~/go/bin、$GOPATH/*/bin）。
fn opencode_extra_search_paths(
    home: &Path,
    opencode_install_dir: Option<std::ffi::OsString>,
    xdg_bin_dir: Option<std::ffi::OsString>,
    gopath: Option<std::ffi::OsString>,
) -> Vec<std::path::PathBuf> {
    let mut paths = IndexSet::new();

    push_env_single_dir(&mut paths, opencode_install_dir);
    push_env_single_dir(&mut paths, xdg_bin_dir);

    if !home.as_os_str().is_empty() {
        push_unique_path(&mut paths, home.join("bin"));
        push_unique_path(&mut paths, home.join(".opencode").join("bin"));
        push_unique_path(&mut paths, home.join(".bun").join("bin"));
        push_unique_path(&mut paths, home.join("go").join("bin"));
    }

    extend_from_path_list(&mut paths, gopath, Some("bin"));

    paths.into_iter().collect()
}

/// Grok's official installer writes the launcher to `$GROK_BIN_DIR` or, by
/// default, `~/.grok/bin`. Keep these ahead of generic npm/Node locations so
/// version probing and anchored updates can see the native distribution even
/// when the GUI process inherited a stale PATH.
fn grok_extra_search_paths(
    home: &Path,
    grok_bin_dir: Option<std::ffi::OsString>,
) -> Vec<std::path::PathBuf> {
    let mut paths = IndexSet::new();
    push_env_single_dir(&mut paths, grok_bin_dir);
    if !home.as_os_str().is_empty() {
        push_unique_path(&mut paths, home.join(".grok").join("bin"));
    }
    paths.into_iter().collect()
}

/// MiniMax Code 官方 installer 的入口：POSIX 在 `<root>/bin/mcode`，Windows 的
/// `mcode.cmd` / `mcode.ps1` 直接放在 `<root>` 下（不在 `bin`）。root 默认
/// `~/.minimax-code`，可由 `MCODE_INSTALL_DIR` 改。installer 只改 shell rc / 用户 PATH，
/// GUI 进程继承的旧 PATH 看不到，所以同 Grok 一样显式排在通用 npm/Node 目录前面。
fn mcode_extra_search_paths(
    home: &Path,
    mcode_install_dir: Option<std::ffi::OsString>,
) -> Vec<std::path::PathBuf> {
    let launcher_dir = |root: std::path::PathBuf| {
        if cfg!(target_os = "windows") {
            root
        } else {
            root.join("bin")
        }
    };
    let mut paths = IndexSet::new();
    if let Some(root) = mcode_install_dir.filter(|value| !value.is_empty()) {
        push_unique_path(&mut paths, launcher_dir(std::path::PathBuf::from(root)));
    }
    if !home.as_os_str().is_empty() {
        push_unique_path(&mut paths, launcher_dir(home.join(".minimax-code")));
    }
    paths.into_iter().collect()
}

/// PATHEXT 里 `.CMD` 是否排在 `.EXE` 之前。未设置或两者都不在时按 Windows 默认
/// （`.EXE` 在前）处理。
#[cfg(target_os = "windows")]
fn pathext_prefers_cmd_over_exe(pathext: Option<&str>) -> bool {
    let Some(pathext) = pathext else {
        return false;
    };
    let position = |ext: &str| {
        pathext
            .split(';')
            .map(str::trim)
            .position(|entry| entry.eq_ignore_ascii_case(ext))
    };
    match (position(".cmd"), position(".exe")) {
        (Some(cmd), Some(exe)) => cmd < exe,
        (Some(_), None) => true,
        _ => false,
    }
}

/// 同一目录里 `<tool>.cmd` 和 `<tool>.exe` 并存时，命令行敲 `<tool>` 只会按 PATHEXT
/// 顺序命中其中一个，另一个被遮住。调用方只对已知是同一处安装多出来的入口用它
/// （Hermes 换成 `.exe` 启动器后留下的旧 `hermes.cmd`，#8839）。
#[cfg(target_os = "windows")]
fn shadowed_by_pathext_sibling(path: &Path, pathext: Option<&str>) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let cmd_first = pathext_prefers_cmd_over_exe(pathext);
    let shadowing_ext = if ext.eq_ignore_ascii_case("cmd") && !cmd_first {
        "exe"
    } else if ext.eq_ignore_ascii_case("exe") && cmd_first {
        "cmd"
    } else {
        return false;
    };
    path.with_extension(shadowing_ext).is_file()
}

fn tool_executable_candidates(tool: &str, dir: &Path) -> Vec<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let extensionless = dir.join(tool);
        let exe = dir.join(format!("{tool}.exe"));
        let mut candidates = vec![dir.join(format!("{tool}.cmd"))];
        if let Some(exe) = windows_preferred_tool_executable(tool, &exe) {
            candidates.push(exe);
        }
        if windows_runnable_sibling_for_extensionless_tool(&extensionless).is_none() {
            candidates.push(extensionless);
        }
        candidates
    }

    #[cfg(not(target_os = "windows"))]
    {
        vec![dir.join(tool)]
    }
}

fn extend_mise_node_search_paths(paths: &mut IndexSet<std::path::PathBuf>, home: &Path) {
    if home.as_os_str().is_empty() {
        return;
    }

    let mise_base = home.join(".local/share/mise");
    push_unique_path(paths, mise_base.join("shims"));

    let node_installs = mise_base.join("installs").join("node");
    if node_installs.exists() {
        if let Ok(entries) = std::fs::read_dir(&node_installs) {
            for entry in entries.flatten() {
                let bin_path = entry.path().join("bin");
                if bin_path.exists() {
                    push_unique_path(paths, bin_path);
                }
            }
        }
    }
}

/// The "effective PATH" used for CLI detection and lifecycle commands. On Windows
/// the inherited process PATH can be incomplete — most notably after an in-app self-update,
/// where the MSI/WiX-auto-launched process inherits only the machine-level PATH
/// and drops the user-level PATH (see #6061). Any CLI installed in a user-PATH
/// location (winget Claude `%LOCALAPPDATA%\Programs\claude`, the standalone
/// Codex installer `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`, a custom npm
/// prefix `D:\npm-global`, …) then reads as "not installed".
///
/// Reconstruct the effective PATH by merging the process PATH with the machine
/// and user registry PATH values (`REG_EXPAND_SZ` expanded), so detection sees
/// the same installations a freshly logged-in shell would — regardless of how
/// the current process was launched. Process entries are kept first (a runtime
/// override wins); registry entries fill whatever the process is missing;
/// duplicates are removed.
///
/// See `env_checker::check_system_env` for the same set of registry keys; here
/// referenced variables also fall back to these keys if absent from the process.
///
/// fnm's default Node alias is appended last (see `fnm_default_alias_dir_candidates`),
/// so a Node that only exists in `fnm env` shells is still visible, while every
/// explicit PATH entry keeps precedence.
#[cfg(target_os = "windows")]
fn effective_path_string() -> String {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let process = std::env::var_os("PATH")
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let user = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Environment")
        .and_then(|k| k.get_value::<String, &str>("Path"))
        .map(|raw| expand_env_chars(&raw))
        .unwrap_or_default();
    let machine = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment")
        .and_then(|k| k.get_value::<String, &str>("Path"))
        .map(|raw| expand_env_chars(&raw))
        .unwrap_or_default();
    let fnm = fnm_default_alias_dir_candidates(
        std::env::var_os("FNM_DIR"),
        &dirs::home_dir().unwrap_or_default(),
        dirs::data_dir(),
    )
    .into_iter()
    .filter(|dir| dir.is_dir())
    .map(|dir| dir.to_string_lossy().into_owned())
    .collect::<Vec<_>>()
    .join(";");
    merge_path_segments_win(&[&process, &user, &machine, &fnm])
}

/// fnm only puts Node on PATH through `fnm env`, which prepends a per-shell
/// `%LOCALAPPDATA%\fnm_multishells\<id>` link to that shell's PATH. Nothing is
/// written to the registry PATH, so a GUI-launched CC Switch sees neither node,
/// npm nor the npm global CLIs (#8484, #6991). `<fnm dir>\aliases\default` is a
/// stable link to the default Node installation; on Windows node.exe, npm.cmd
/// and npm global commands live directly in that directory (no `bin`).
///
/// Mirrors fnm's own base-dir lookup: `FNM_DIR`, then the legacy `~\.fnm`, then
/// `%APPDATA%\fnm`. Callers keep only the directories that exist.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn fnm_default_alias_dir_candidates(
    fnm_dir: Option<std::ffi::OsString>,
    home: &Path,
    data_dir: Option<std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    let mut bases = IndexSet::new();
    if let Some(dir) = fnm_dir.filter(|value| !value.is_empty()) {
        push_unique_path(&mut bases, std::path::PathBuf::from(dir));
    }
    if !home.as_os_str().is_empty() {
        push_unique_path(&mut bases, home.join(".fnm"));
    }
    if let Some(data_dir) = data_dir {
        push_unique_path(&mut bases, data_dir.join("fnm"));
    }
    bases
        .into_iter()
        .map(|base| base.join("aliases").join("default"))
        .collect()
}

/// `extend_from_cli_path_env` consumes an `OsString` via `split_paths`. On
/// Windows this is derived from `effective_path_string`; on other platforms the
/// raw process value is returned unchanged (zero behaviour change).
#[cfg(target_os = "windows")]
fn effective_path_os() -> Option<std::ffi::OsString> {
    Some(std::ffi::OsString::from(effective_path_string()))
}

#[cfg(not(target_os = "windows"))]
fn effective_path_os() -> Option<std::ffi::OsString> {
    std::env::var_os("PATH")
}

/// Prepend a candidate directory without converting the existing PATH to
/// UTF-8. Unix permits arbitrary non-NUL bytes in environment values; keeping
/// this as an `OsString` ensures one non-Unicode segment cannot discard or
/// corrupt every other interpreter directory needed by an npm/python shim.
#[cfg(not(target_os = "windows"))]
fn prepend_search_dir_to_path(dir: &Path, current_path: &std::ffi::OsStr) -> std::ffi::OsString {
    let mut path = dir.as_os_str().to_os_string();
    if !current_path.is_empty() {
        path.push(":");
        path.push(current_path);
    }
    path
}

/// Expand `%VAR%` environment-variable references. The registry `Path` value is
/// `REG_EXPAND_SZ`, and the `String` returned by `winreg` is not auto-expanded.
/// MSI launches can also lose user-defined variables such as `%PNPM_HOME%`.
/// Prefer process overrides, then the user and machine registry values. Registry
/// values may reference other variables; unknown/cyclic references stay literal.
#[cfg(target_os = "windows")]
fn expand_env_chars(raw: &str) -> String {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let user = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Environment")
        .ok();
    let machine = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment")
        .ok();
    expand_env_chars_with_lookup(
        raw,
        &|name| std::env::var(name).ok(),
        &|name| {
            user.as_ref()
                .and_then(|key| key.get_value::<String, _>(name).ok())
                .or_else(|| {
                    machine
                        .as_ref()
                        .and_then(|key| key.get_value::<String, _>(name).ok())
                })
        },
        &mut Vec::new(),
    )
}

#[cfg(target_os = "windows")]
fn expand_env_chars_with_lookup(
    raw: &str,
    process: &impl Fn(&str) -> Option<String>,
    registry: &impl Fn(&str) -> Option<String>,
    expanding: &mut Vec<String>,
) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('%') {
            None => {
                out.push('%');
                out.push_str(after);
                rest = "";
                break;
            }
            Some(close) => {
                let name = &after[..close];
                let is_ident =
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                let value = if is_ident {
                    process(name).or_else(|| {
                        // Bound nesting even for an unusually long registry chain.
                        if expanding.len() >= 32
                            || expanding.iter().any(|var| var.eq_ignore_ascii_case(name))
                        {
                            return None;
                        }
                        let raw = registry(name)?;
                        expanding.push(name.to_owned());
                        let value =
                            expand_env_chars_with_lookup(&raw, process, registry, expanding);
                        expanding.pop();
                        Some(value)
                    })
                } else {
                    None
                };
                match value {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[close + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Merge several Windows PATH strings (`;`-separated) in order, keeping only
/// the first occurrence of each segment (case-insensitive). Process segments
/// come first to respect runtime overrides, followed by user- and
/// machine-level registry segments.
#[cfg(target_os = "windows")]
fn merge_path_segments_win(parts: &[&str]) -> String {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut merged: Vec<&str> = Vec::new();
    for part in parts {
        for seg in part.split(';') {
            let s = seg.trim();
            if s.is_empty() || !seen.insert(s.to_ascii_lowercase()) {
                continue;
            }
            merged.push(s);
        }
    }
    merged.join(";")
}

/// 构建某工具的候选搜索目录（原生安装优先，PATH 兜底）。
/// 单探兜底 (`scan_cli_version`) 与全量枚举 (`enumerate_tool_installations`) 共用，
/// 确保两条路径看到的是同一组安装位置。
fn build_tool_search_paths(tool: &str) -> Vec<std::path::PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();

    // 常见的安装路径（原生安装优先）
    // Preserve search priority without scanning all previous paths for every insertion.
    let mut search_paths = IndexSet::new();
    if tool == "kimi" {
        let install_dir = std::env::var_os("KIMI_INSTALL_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".kimi-code"));
        push_unique_path(&mut search_paths, install_dir.join("bin"));
    }
    if tool == "grok" {
        let extra_paths = grok_extra_search_paths(&home, std::env::var_os("GROK_BIN_DIR"));
        for path in extra_paths {
            push_unique_path(&mut search_paths, path);
        }
    }
    if tool == "mcode" {
        let extra_paths = mcode_extra_search_paths(&home, std::env::var_os("MCODE_INSTALL_DIR"));
        for path in extra_paths {
            push_unique_path(&mut search_paths, path);
        }
    }
    if tool == "omp" {
        // A Bun global install (`bun install -g @oh-my-pi/pi-coding-agent`) puts
        // the launcher in Bun's global bin, which is not on the PATH a GUI process
        // inherits — same reason OpenCode scans it. The official installer's
        // prebuilt binary (`~/.local/bin`) and Homebrew are covered below.
        if !home.as_os_str().is_empty() {
            push_unique_path(&mut search_paths, home.join(".bun").join("bin"));
        }
    }
    if !home.as_os_str().is_empty() {
        push_unique_path(&mut search_paths, home.join(".local/bin"));
        push_unique_path(&mut search_paths, home.join(".npm-global/bin"));
        push_unique_path(&mut search_paths, home.join("n/bin"));
        push_unique_path(&mut search_paths, home.join(".volta/bin"));
        extend_mise_node_search_paths(&mut search_paths, &home);
    }

    #[cfg(target_os = "macos")]
    {
        push_unique_path(
            &mut search_paths,
            std::path::PathBuf::from("/opt/homebrew/bin"),
        );
        push_unique_path(
            &mut search_paths,
            std::path::PathBuf::from("/usr/local/bin"),
        );
        if tool == "hermes" {
            let python_base = home.join("Library").join("Python");
            if python_base.exists() {
                if let Ok(entries) = std::fs::read_dir(&python_base) {
                    for entry in entries.flatten() {
                        let bin_path = entry.path().join("bin");
                        if bin_path.exists() {
                            push_unique_path(&mut search_paths, bin_path);
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        push_unique_path(
            &mut search_paths,
            std::path::PathBuf::from("/usr/local/bin"),
        );
        push_unique_path(&mut search_paths, std::path::PathBuf::from("/usr/bin"));
    }

    #[cfg(target_os = "windows")]
    {
        // Official standalone (non-npm) installer locations — belt-and-suspenders
        // alongside the registry-PATH merge in `effective_path_os`. These
        // installers normally register themselves on the user PATH, but some
        // per-user MSI/MSIX installs do not, and an in-app-update relaunch can
        // drop the user PATH (#6061), so add them explicitly here. Placed ahead
        // of the npm directory so a native install wins over a stale npm shim
        // (#4701).
        if let Some(local_data) = dirs::data_local_dir() {
            if tool == "codex" {
                // OpenAI Codex Installer.exe / .msi standalone install location
                // (#6061, #6047).
                push_unique_path(
                    &mut search_paths,
                    local_data
                        .join("Programs")
                        .join("OpenAI")
                        .join("Codex")
                        .join("bin"),
                );
            }
            if tool == "claude" {
                // `winget install Anthropic.ClaudeCode` / official native
                // installer location (#6278).
                push_unique_path(
                    &mut search_paths,
                    local_data.join("Programs").join("claude"),
                );
            }
            if tool == "agy" {
                // Antigravity CLI official install.ps1 location.
                push_unique_path(&mut search_paths, local_data.join("agy").join("bin"));
            }
        }
        if let Some(appdata) = dirs::data_dir() {
            push_unique_path(&mut search_paths, appdata.join("npm"));
            if tool == "hermes" {
                let python_base = appdata.join("Python");
                if python_base.exists() {
                    if let Ok(entries) = std::fs::read_dir(&python_base) {
                        for entry in entries.flatten() {
                            let scripts_path = entry.path().join("Scripts");
                            if scripts_path.exists() {
                                push_unique_path(&mut search_paths, scripts_path);
                            }
                        }
                    }
                }
            }
        }
        if tool == "hermes" {
            if let Some(local_data) = dirs::data_local_dir() {
                let programs_python = local_data.join("Programs").join("Python");
                if programs_python.exists() {
                    if let Ok(entries) = std::fs::read_dir(&programs_python) {
                        for entry in entries.flatten() {
                            let scripts_path = entry.path().join("Scripts");
                            if scripts_path.exists() {
                                push_unique_path(&mut search_paths, scripts_path);
                            }
                        }
                    }
                }
            }
        }
        push_unique_path(
            &mut search_paths,
            std::path::PathBuf::from("C:\\Program Files\\nodejs"),
        );
        extend_windows_cli_manager_search_paths(&mut search_paths, &home);
    }

    let fnm_base = home.join(".local/state/fnm_multishells");
    if fnm_base.exists() {
        if let Ok(entries) = std::fs::read_dir(&fnm_base) {
            for entry in entries.flatten() {
                let bin_path = entry.path().join("bin");
                if bin_path.exists() {
                    push_unique_path(&mut search_paths, bin_path);
                }
            }
        }
    }

    let nvm_base = home.join(".nvm/versions/node");
    if nvm_base.exists() {
        if let Ok(entries) = std::fs::read_dir(&nvm_base) {
            for entry in entries.flatten() {
                let bin_path = entry.path().join("bin");
                if bin_path.exists() {
                    push_unique_path(&mut search_paths, bin_path);
                }
            }
        }
    }

    if tool == "opencode" {
        let extra_paths = opencode_extra_search_paths(
            &home,
            std::env::var_os("OPENCODE_INSTALL_DIR"),
            std::env::var_os("XDG_BIN_DIR"),
            std::env::var_os("GOPATH"),
        );

        for path in extra_paths {
            push_unique_path(&mut search_paths, path);
        }
    }

    let path_env = effective_path_os();
    extend_from_cli_path_env(&mut search_paths, path_env);
    search_paths.into_iter().collect()
}

#[cfg(target_os = "windows")]
fn is_windows_command_script(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        .unwrap_or(false)
}

/// Convert a canonicalized Windows path back to the form accepted by shell
/// commands. `std::fs::canonicalize` prefixes local paths with `\\?\` (and UNC
/// paths with `\\?\UNC\`), but `cmd.exe` cannot `call` a batch file through
/// those verbatim paths and reports "The system cannot find the path
/// specified." Direct Win32 executable launches accept the prefix; batch
/// scripts do not.
#[cfg(target_os = "windows")]
fn windows_shell_compatible_path(path: &Path) -> std::path::PathBuf {
    let raw = path.to_string_lossy();
    if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        std::path::PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(local) = raw.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(local)
    } else {
        path.to_path_buf()
    }
}

#[cfg(target_os = "windows")]
fn windows_runnable_sibling_for_extensionless_tool(path: &Path) -> Option<std::path::PathBuf> {
    if path.extension().is_some() {
        return None;
    }

    ["cmd", "exe"]
        .iter()
        .map(|ext| path.with_extension(ext))
        .find(|candidate| candidate.is_file())
}

/// OpenCode Desktop has shipped both root-level and Electron `resources/` CLI sidecars.
/// Some Electron releases omitted the sidecar; `resources/app.asar` still identifies the
/// Desktop bundle, so skip its GUI instead of probing `opencode.exe` with `--version`.
/// A standalone OpenCode CLI install remains valid when no Desktop marker is present.
#[cfg(target_os = "windows")]
fn windows_preferred_tool_executable(tool: &str, path: &Path) -> Option<std::path::PathBuf> {
    if tool == "opencode"
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("opencode.exe"))
    {
        if let Some(parent) = path.parent() {
            let sidecar = parent.join("opencode-cli.exe");
            if sidecar.is_file() {
                return Some(sidecar);
            }

            let resources = parent.join("resources");
            let bundled_sidecar = resources.join("opencode-cli.exe");
            if bundled_sidecar.is_file() {
                return Some(bundled_sidecar);
            }

            if resources.join("app.asar").is_file() {
                return None;
            }
        }
    }

    Some(
        windows_runnable_sibling_for_extensionless_tool(path).unwrap_or_else(|| path.to_path_buf()),
    )
}

#[cfg(target_os = "windows")]
fn windows_preferred_path_default(tool: &str, raw: &str) -> Option<std::path::PathBuf> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !is_windows_app_execution_alias_dir(Path::new(line).parent().unwrap_or(Path::new("")))
        })
        .find_map(|line| windows_preferred_tool_executable(tool, Path::new(line)))
}

#[cfg(target_os = "windows")]
fn build_windows_tool_command(
    tool_path: &Path,
    args: &[&str],
    new_path: &str,
) -> std::process::Command {
    use std::process::Command;

    if is_windows_command_script(tool_path) {
        // `resolve_path_default` returns a canonical path so callers can
        // compare installation identities. Canonical Windows paths carry a
        // `\\?\` prefix, which `cmd /C call` rejects for batch files. Normalize
        // only at this shell boundary and keep the canonical identity intact
        // everywhere else.
        let shell_path = windows_shell_compatible_path(tool_path);
        let path = shell_path.to_string_lossy();
        let args = args
            .iter()
            .map(|arg| windows_cmd_double_quote_arg(arg))
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "call {}{}",
            win_quote_path_for_batch(&path),
            if args.is_empty() {
                String::new()
            } else {
                format!(" {args}")
            }
        );
        let mut cmd = Command::new("cmd");
        cmd.args(["/D", "/S", "/C"])
            .raw_arg(&command)
            .env("PATH", new_path)
            .creation_flags(CREATE_NO_WINDOW);
        return cmd;
    }

    let mut cmd = Command::new(tool_path);
    cmd.args(args)
        .env("PATH", new_path)
        .creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(target_os = "windows")]
fn run_windows_tool_command(
    tool_path: &Path,
    args: &[&str],
    new_path: &str,
) -> std::io::Result<std::process::Output> {
    build_windows_tool_command(tool_path, args, new_path).output()
}

#[cfg(target_os = "windows")]
fn run_windows_tool_version_command(
    tool_path: &Path,
    new_path: &str,
) -> std::io::Result<std::process::Output> {
    run_windows_tool_command(tool_path, &["--version"], new_path)
}

/// Probe the version of the PATH-default entry on Windows. Uses
/// `resolve_path_default` (which merges the registry PATH and filters out
/// App Execution Aliases) to get the executable the user actually runs, then
/// runs `--version` on it. Consumed by `get_single_tool_version_impl` before
/// the directory scan, so the displayed version matches what `tool` resolves
/// to in a terminal (#4701).
///
/// Returns `NotFound` when no entry is resolved on PATH, or the resolved entry
/// fails to launch (the caller falls back to `scan_cli_version` over the
/// hardcoded/registry dirs, which still reports it as broken when nothing else
/// runs); `FoundButFailed` when an entry ran but no version came out of it —
/// it exited non-zero or printed nothing (installed but not runnable).
/// That is reported as-is without falling back, so an old
/// install elsewhere cannot mask a broken default.
#[cfg(target_os = "windows")]
fn probe_path_default_version(tool: &str) -> ShellProbe {
    let path_default = match resolve_path_default(tool, None) {
        Ok(Some(p)) => p,
        _ => return ShellProbe::NotFound(NOT_INSTALLED.to_string()),
    };
    let current_path = effective_path_string();
    match run_windows_tool_version_command(&path_default, &current_path) {
        Ok(out) => {
            let stdout = decode_command_output(&out.stdout).trim().to_string();
            let stderr = decode_command_output(&out.stderr).trim().to_string();
            if out.status.success() {
                let raw = if stdout.is_empty() { &stderr } else { &stdout };
                if raw.is_empty() {
                    ShellProbe::FoundButFailed(silent_version_probe_detail(out.status))
                } else {
                    ShellProbe::Found(extract_version(raw))
                }
            } else {
                let err = if stderr.is_empty() { stdout } else { stderr };
                if err.is_empty() {
                    ShellProbe::FoundButFailed(silent_version_probe_detail(out.status))
                } else {
                    ShellProbe::FoundButFailed(last_lines(err.trim(), 4))
                }
            }
        }
        // 启动不了不一定是装坏了：`where` 会列出 npm 留下的无扩展名 sh shim，同目录
        // 没有 .cmd 时它可能排在最前，而终端会跳过它、去跑后面那份。交给目录扫描判断：
        // 别处有能跑的就以它为准，都跑不起来扫描也会报「装了但跑不起来」。
        Err(_) => ShellProbe::NotFound(NOT_INSTALLED.to_string()),
    }
}

/// 扫描常见路径查找 CLI（PATH 主命令未命中时的兜底单探）。
fn scan_cli_version(tool: &str) -> ShellProbe {
    scan_cli_version_in(tool, &build_tool_search_paths(tool))
}

fn scan_cli_version_in(tool: &str, search_paths: &[std::path::PathBuf]) -> ShellProbe {
    #[cfg(not(target_os = "windows"))]
    use std::process::Command;

    #[cfg(target_os = "windows")]
    let current_path = effective_path_string();
    #[cfg(not(target_os = "windows"))]
    let current_path = effective_path_os().unwrap_or_default();

    // 记录"可执行文件存在、但 `--version` 非零退出"时的首个诊断信息。
    // 典型场景：工具已安装但当前环境跑不起来（如 openclaw 要求 Node v22.19+）。
    // 这类信息比笼统的 "not installed" 有用得多，循环结束未探到版本时回传。
    let mut exec_diagnostic: Option<String> = None;
    // 文件在、却启动失败或没有任何输出时的首个诊断。没有更具体的报错时才用它：
    // 文件在就不算没装（安装位置列表也会列出它）。
    let mut silent_diagnostic: Option<String> = None;

    for path in search_paths {
        #[cfg(target_os = "windows")]
        let new_path = format!("{};{}", path.display(), current_path);

        #[cfg(not(target_os = "windows"))]
        let new_path = prepend_search_dir_to_path(path, &current_path);

        for tool_path in tool_executable_candidates(tool, path) {
            // 只认文件：与工具同名的目录不是安装，否则启动失败会被当成「装了跑不起来」
            if !tool_path.is_file() {
                continue;
            }

            #[cfg(target_os = "windows")]
            let output = run_windows_tool_version_command(&tool_path, &new_path);

            #[cfg(not(target_os = "windows"))]
            let output = {
                Command::new(&tool_path)
                    .arg("--version")
                    .env("PATH", &new_path)
                    .output()
            };

            let out = match output {
                Ok(out) => out,
                Err(e) => {
                    silent_diagnostic.get_or_insert_with(|| launch_failure_detail(&tool_path, &e));
                    continue;
                }
            };
            let stdout = decode_command_output(&out.stdout).trim().to_string();
            let stderr = decode_command_output(&out.stderr).trim().to_string();
            if out.status.success() {
                let raw = if stdout.is_empty() { &stderr } else { &stdout };
                if !raw.is_empty() {
                    return ShellProbe::Found(extract_version(raw));
                }
                silent_diagnostic.get_or_insert_with(|| silent_version_probe_detail(out.status));
            } else {
                let detail = if stderr.is_empty() { stdout } else { stderr };
                let detail = detail.trim();
                if detail.is_empty() {
                    silent_diagnostic
                        .get_or_insert_with(|| silent_version_probe_detail(out.status));
                } else if exec_diagnostic.is_none() {
                    exec_diagnostic = Some(last_lines(detail, 4));
                }
            }
        }
    }

    // 有诊断 = 找到了可执行文件但拿不到版本（装了跑不起来），具体报错优先；否则视作未安装。
    match exec_diagnostic.or(silent_diagnostic) {
        Some(detail) => ShellProbe::FoundButFailed(detail),
        None => ShellProbe::NotFound(NOT_INSTALLED.to_string()),
    }
}

/// 单个工具在系统中的一处安装，用于"多处安装互相打架"的冲突诊断。
/// 字段保持 snake_case（与 `ToolVersion` 一致），前端按同名字段读取。
#[derive(Debug, serde::Serialize)]
pub struct ToolInstallation {
    /// 候选入口路径（用户实际在 PATH 里看到/输入的那个，未解析软链）。
    path: String,
    /// `--version` 成功时解析出的版本号。
    version: Option<String>,
    /// `--version` 是否 exit 0（装了且能在当前环境跑起来）。
    runnable: bool,
    /// 跑不起来时的诊断信息末尾若干行。
    error: Option<String>,
    /// `--version` 真的跑完并以非 0 退出（命令自己报错）。超时被击杀、启动失败都不算：
    /// 那只说明这次没探到，不能证明装坏了。Codex 的卸载重装自愈只认这个信号——误判
    /// 健康的一处为损坏、重装又失败时，旧版已被卸掉，用户看到「未安装」。
    #[serde(skip)]
    exited_with_error: bool,
    /// 由路径前缀推断的安装来源（nvm/homebrew/...），驱动 UI 徽章。
    source: String,
    /// 是否为 PATH 解析到的那处（= 命令行默认，也是升级会作用的目标）。
    is_path_default: bool,
    /// canonicalize 解析后的真身路径(brew 形如 `Cellar/<formula>/...`、claude 原生形如
    /// `~/.local/share/claude/versions/...`),用于 `anchored_command_from_paths` 的真身
    /// 判定。`enumerate_tool_installations` 已经为去重算过一次,这里复用避免上游
    /// `installs_anchored_command` 再 canonicalize 一遍——消除冗余 syscall + 闭合
    /// "enumerate 与 anchor 看到同一真身"的一致性边界(否则两次 canonicalize 之间
    /// symlink 被换会让锚定指向不同真身)。`#[serde(skip)]` 不外露给前端。
    #[serde(skip)]
    real: std::path::PathBuf,
}

/// 由可执行文件路径前缀推断安装来源。纯字符串匹配、无副作用。
/// 顺序敏感：Homebrew 的 Cellar / Caskroom 真身要先于通用规则命中。
fn infer_install_source(path: &Path) -> &'static str {
    let s = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if s.contains("/.nvm/") {
        "nvm"
    } else if s.contains("/homebrew/") || s.contains("/cellar/") || s.contains("/caskroom/") {
        "homebrew"
    // `.volta` 是 macOS/Linux 默认安装(`~/.volta/bin`),`/volta/` 兜底覆盖
    // Windows 的 `%LOCALAPPDATA%\Volta\bin` / `%VOLTA_HOME%\bin`(无前导点)。
    } else if s.contains("/.volta/") || s.contains("/volta/") {
        "volta"
    } else if s.contains("fnm_multishells") {
        "fnm"
    } else if s.contains("/mise/") {
        "mise"
    } else if s.contains("/.bun/") {
        "bun"
    // pnpm 全局包目录: macOS 一般 `~/.local/share/pnpm`(已 normalize 到 `/pnpm/`)
    // 与 Windows `%LOCALAPPDATA%\pnpm` / `%PNPM_HOME%` 都命中 `/pnpm/`。
    } else if s.contains("/pnpm/") {
        "pnpm"
    } else if s.contains("/scoop/") {
        "scoop"
    } else if s.contains("/library/python")
        || s.contains("/scripts/")
        || s.contains("/site-packages/")
    {
        "pip"
    } else {
        "system"
    }
}

/// 安装来源：Homebrew 看 canonicalize 真身，node 管理器仍看 launcher。
///
/// Intel Cask 入口是 `/usr/local/bin/codex`，不含 `/homebrew/` / `/caskroom/`，
/// 真身才在 `/usr/local/Caskroom/...`。只看 launcher 会把徽章标成 `system`。
/// nvm/volta/fnm 的分类在 shim 路径上，必须继续用未解析的入口。
fn infer_install_source_for_install(launcher: &Path, real: &Path) -> &'static str {
    let real_s = real
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if real_s.contains("/cellar/") || real_s.contains("/caskroom/") {
        "homebrew"
    } else {
        infer_install_source(launcher)
    }
}

/// 从 shell 输出里挑出第一个绝对路径行（trim 后以 `/` 开头），跳过交互式登录 shell
/// （`-lic`）里 .zshrc 打印的欢迎语/提示符等噪音。canonicalize 由调用方做（碰 FS）。
#[cfg(not(target_os = "windows"))]
fn first_abs_path_line(raw: &str) -> Option<&str> {
    raw.lines().map(str::trim).find(|l| l.starts_with('/'))
}

/// 从 `env` 输出里取 `PATH=` 那行的值。要求值以 `/` 开头——PATH 首段必是绝对路径，
/// 这条约束顺带跳过"某个多行值的环境变量恰好有一行以 `PATH=` 开头"的污染，
/// 与 `first_abs_path_line` 对交互式 shell 噪音的容错同理。
#[cfg(not(target_os = "windows"))]
fn path_line_from_env_output(raw: &str) -> Option<&str> {
    raw.lines()
        .filter_map(|line| line.strip_prefix("PATH="))
        .find(|value| value.starts_with('/'))
}

/// 合并两段 PATH：`primary` 全部保留在前，`extra` 中未出现过的段按序追加。
/// 空段直接丢弃（`a::b` 里的空段在 POSIX 下语义是"当前目录"，注入时不该带上）。
#[cfg(not(target_os = "windows"))]
pub(crate) fn merge_path_segments(primary: &str, extra: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut merged: Vec<&str> = Vec::new();
    for segment in primary.split(':').chain(extra.split(':')) {
        if segment.is_empty() || !seen.insert(segment) {
            continue;
        }
        merged.push(segment);
    }
    merged.join(":")
}

/// 用与 `resolve_path_default` 相同的登录 shell 解析用户的真实 PATH，
/// 供 `run_tool_lifecycle_silently` 注入给安装/升级脚本，也供
/// `run_detected_tool_command_with_timeout` 注入给运行时调用的 CLI。
///
/// **要解决的不对称**：探测阶段（`try_get_version` / `resolve_path_default`）跑的是
/// `$SHELL -lic`，会读 `.zshrc`/`.zprofile`，看得到 nvm / homebrew / volta；而执行阶段
/// 是非登录 `bash -c`，继承的是 launchd 给 GUI App 的 PATH，通常只有
/// `/usr/bin:/bin:/usr/sbin:/sbin`。锚定命令自己用绝对路径调用执行体，本不受这条影响
/// ——但有两类漏网：
/// 1. **执行体在内部再 spawn 第三方 CLI**：`grok update` 靠 `npm view` 查最新版本
///    （见 `grok_native_update_command`），npm 又是 `#!/usr/bin/env node` 脚本；
///    未来任何 self-update 内部调 node/git/python 同理。
/// 2. **install 分支的 `<官方 installer> || npm i -g <pkg>@latest`**：`||` 右侧是裸命令，
///    窄 PATH 下必然 exit 127，等于没有兜底。
///
/// 把执行阶段的 PATH 拉平到探测阶段的水平，一次消除这两类。
///
/// **用 `/usr/bin/env` 而不是 `echo $PATH`**：`$PATH` 在 fish 里是 list 类型，
/// `"$PATH"` 展开成空格分隔而非冒号分隔；`env` 打印的则是子进程的真实环境，
/// 任何 shell 下格式都正确。写绝对路径又绕过了 alias / function / PATH 三重不确定性
/// （交互式 shell 会加载用户 alias）。
///
/// 解析不到（含超过 `deadline`）时返回 `None`，调用方保持原有行为（不注入），
/// 不引入新的失败模式。
#[cfg(not(target_os = "windows"))]
pub(crate) fn login_shell_path(deadline: Option<CommandDeadline>) -> Option<String> {
    use std::process::{Command, Stdio};
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| is_valid_shell(s))
        .unwrap_or_else(|| "sh".to_string());
    let flag = default_flag_for_shell(&shell);
    let mut cmd = Command::new(shell);
    cmd.arg(flag)
        .arg("/usr/bin/env")
        // 与 `resolve_path_default` 相同：交互式 rc 不能读到继承来的 stdin，
        // 也不能因持有控制终端被作业控制停住。
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_child_process_group(&mut cmd);
    let child = cmd.spawn().ok()?;
    let out = wait_child_output(child, deadline).ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = decode_command_output(&out.stdout);
    Some(path_line_from_env_output(&raw)?.to_string())
}

/// 用与 `try_get_version` 相同的登录 shell 解析 PATH 默认命中的可执行文件路径，
/// canonicalize 后作为"命令行默认 / 升级目标"的锚点（与升级会作用的那处对齐）。
#[cfg(not(target_os = "windows"))]
fn resolve_path_default(
    tool: &str,
    deadline: Option<CommandDeadline>,
) -> Result<Option<std::path::PathBuf>, String> {
    use std::process::{Command, Stdio};

    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| is_valid_shell(s))
        .unwrap_or_else(|| "sh".to_string());
    let flag = default_flag_for_shell(&shell);
    let mut cmd = Command::new(shell);
    cmd.arg(flag)
        .arg(format!("command -v {tool}"))
        // 改 spawn 后 stdin 不再像 output() 那样默认置 null，须显式关闭：
        // 继承来的 stdin 可能是终端/管道，交互式 rc 里的读操作会永久阻塞。
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_child_process_group(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to locate {tool}: {e}"))?;
    let out = wait_child_output(child, deadline)?;
    if !out.status.success() {
        return Ok(None);
    }
    let raw = decode_command_output(&out.stdout);
    // 不能死取第一行：交互式 .zshrc 可能先打印欢迎语（如 "🚀 Welcome back"），
    // command -v 的真实路径在其后；取第一个 `/` 开头的行才稳。
    let Some(first) = first_abs_path_line(&raw) else {
        return Ok(None);
    };
    Ok(std::fs::canonicalize(first).ok())
}

#[cfg(target_os = "windows")]
fn windows_path_lookup_command(
    tool: &str,
    effective_path: &std::ffi::OsStr,
) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    // Use the system copy explicitly so a project-local `where.exe` cannot
    // hijack the passive lookup before the PATH-only pattern is evaluated.
    let where_exe = PathBuf::from(
        std::env::var_os("SystemRoot").unwrap_or_else(|| std::ffi::OsString::from(r"C:\Windows")),
    )
    .join("System32")
    .join("where.exe");
    let mut command = Command::new(where_exe);
    command
        // `$PATH:pattern` is where.exe's documented environment-variable
        // search form. Unlike a bare pattern, it does not search the current
        // directory before PATH.
        .arg(format!("$PATH:{tool}"))
        .env("PATH", effective_path)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[cfg(target_os = "windows")]
fn resolve_path_default(
    tool: &str,
    deadline: Option<CommandDeadline>,
) -> Result<Option<std::path::PathBuf>, String> {
    // Restrict `where` to the merged effective PATH. A bare `where {tool}` also
    // searches the current directory first, which would let a project-local
    // `codex.cmd` be executed by a passive version check. The `$PATH:pattern`
    // form searches only the supplied environment variable while still seeing
    // registry PATH entries lost by an in-app-update relaunch (#6061).
    let current_path = effective_path_os().unwrap_or_default();
    let child = windows_path_lookup_command(tool, &current_path)
        .spawn()
        .map_err(|e| format!("Failed to locate {tool}: {e}"))?;
    let out = wait_child_output(child, deadline)?;
    if !out.status.success() {
        return Ok(None);
    }
    let raw = decode_command_output(&out.stdout);
    // `where` lists every match on PATH in order. Skip entries that cannot be
    // safely probed (App Execution Aliases and Desktop GUI executables) and keep
    // looking so a later real CLI can still be the PATH default.
    let Some(preferred) = windows_preferred_path_default(tool, &raw) else {
        return Ok(None);
    };
    Ok(std::fs::canonicalize(preferred).ok())
}

/// 升级预检/冲突诊断的单条子进程探测预算。枚举会对每个工具开一次登录 shell、对每处
/// 安装跑一次 `--version`，任何一条挂死（.zshrc 阻塞、nvm shim 指向已删除的 node 等）
/// 都会卡住整个"全部升级"预检——到点整组击杀，该条按探测失败降级，预检继续。
const INSTALL_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 带超时的 `--version` 探测（非 Windows）。与 `scan_cli_version` 的裸 `output()`
/// 不同：stdin 显式置 null、经 `isolate_child_process_group` 进独立会话，超时可整组击杀。
/// `new_path` 取 `&OsStr` 而非 `&str`：与 `prepend_search_dir_to_path` 同一约定，
/// 非 UTF-8 的 PATH 段不能在传递途中被有损转换丢弃。
#[cfg(not(target_os = "windows"))]
fn run_probe_version_command(
    tool_path: &Path,
    new_path: &std::ffi::OsStr,
) -> Result<std::process::Output, String> {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new(tool_path);
    cmd.arg("--version")
        .env("PATH", new_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_child_process_group(&mut cmd);
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    wait_child_output(
        child,
        CommandDeadline::from_timeout(Some(INSTALL_PROBE_TIMEOUT)),
    )
}

/// 带超时的 `--version` 探测（Windows）。命令构造与 `run_windows_tool_version_command`
/// 完全一致（.cmd/.bat 经 `cmd /C call`、其余直连），但改 spawn + `wait_child_output`：
/// 挂死的 .cmd shim / CLI 到点由 `terminate_child_tree`（taskkill /T /F）整树击杀，
/// 预检不再被单个候选卡死。scan 路径保持原 helper 不受影响。
#[cfg(target_os = "windows")]
fn run_probe_version_command(
    tool_path: &Path,
    new_path: &str,
) -> Result<std::process::Output, String> {
    use std::process::Stdio;

    let mut cmd = build_windows_tool_command(tool_path, &["--version"], new_path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| e.to_string())?;
    wait_child_output(
        child,
        CommandDeadline::from_timeout(Some(INSTALL_PROBE_TIMEOUT)),
    )
}

/// 枚举工具在系统中的所有安装（不短路）。与 `scan_cli_version` 共用
/// `build_tool_search_paths`，但不在首个命中处停止——而是对每个去重后的真实
/// 可执行文件都跑一次 `--version`，从而能发现"升级写入 A 处、PATH 实际用 B 处"。
fn enumerate_tool_installations(tool: &str) -> Vec<ToolInstallation> {
    let search_paths = build_tool_search_paths(tool);
    #[cfg(target_os = "windows")]
    let current_path = effective_path_string();
    #[cfg(not(target_os = "windows"))]
    let current_path = effective_path_os().unwrap_or_default();
    // 必须带超时：deadline 传 None 时 wait_child_output 无限等，登录 shell 一挂
    // 整个预检就永久卡死（且前端此阶段无任何反馈）。定位失败仅丢失 is_path_default
    // 标记，非致命。
    let path_default = resolve_path_default(
        tool,
        CommandDeadline::from_timeout(Some(INSTALL_PROBE_TIMEOUT)),
    )
    .ok()
    .flatten();

    let mut seen: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    let mut installs: Vec<ToolInstallation> = Vec::new();
    #[cfg(target_os = "windows")]
    let pathext = std::env::var("PATHEXT").ok();

    for dir in &search_paths {
        #[cfg(target_os = "windows")]
        let new_path = format!("{};{}", dir.display(), current_path);
        #[cfg(not(target_os = "windows"))]
        let new_path = prepend_search_dir_to_path(dir, &current_path);

        for tool_path in tool_executable_candidates(tool, dir) {
            // 与 scan_cli_version 一致，只认文件，同名目录不算一处安装
            if !tool_path.is_file() {
                continue;
            }
            // Hermes 发布启动器时换成 .exe 不会删掉旧的 .cmd，两者同在它自己的 bin 目录，
            // 只算命令行实际命中的那个（#8839）。其他工具同目录并存可能是两处真实安装
            // （如 npm prefix 与原生安装同目录），保持分开列出。
            #[cfg(target_os = "windows")]
            if tool == "hermes" && shadowed_by_pathext_sibling(&tool_path, pathext.as_deref()) {
                continue;
            }
            // canonicalize 解析软链后去重：/opt/homebrew/bin/x → Cellar/...、nvm shim 等
            // 多个入口可能指向同一真实文件，只算一处安装。
            let real = std::fs::canonicalize(&tool_path).unwrap_or_else(|_| tool_path.clone());
            if !seen.insert(real.clone()) {
                continue;
            }

            let output = run_probe_version_command(&tool_path, &new_path);

            let (version, runnable, error, exited_with_error) = match output {
                Ok(out) if out.status.success() => {
                    let stdout = decode_command_output(&out.stdout).trim().to_string();
                    let stderr = decode_command_output(&out.stderr).trim().to_string();
                    let raw = if stdout.is_empty() { stderr } else { stdout };
                    (Some(extract_version(&raw)), true, None, false)
                }
                Ok(out) => {
                    let stderr = decode_command_output(&out.stderr).trim().to_string();
                    let stdout = decode_command_output(&out.stdout).trim().to_string();
                    let detail = if stderr.is_empty() { stdout } else { stderr };
                    let detail = detail.trim();
                    let error = if detail.is_empty() {
                        None
                    } else {
                        Some(last_lines(detail, 4))
                    };
                    (None, false, error, true)
                }
                Err(e) => (None, false, Some(e), false),
            };

            let is_path_default = path_default.as_ref() == Some(&real);
            let path_str = tool_path.display().to_string();
            let source = infer_install_source_for_install(&tool_path, &real);

            installs.push(ToolInstallation {
                path: path_str,
                version,
                runnable,
                error,
                exited_with_error,
                source: source.to_string(),
                is_path_default,
                // 复用上面 line ~1357 已 canonicalize 的真身,避免下游
                // installs_anchored_command 再 canonicalize 一遍同一文件。
                real: real.clone(),
            });
        }
    }

    // PATH 默认那处排最前，UI 一眼看到"命令行默认用的是哪处"。
    installs.sort_by_key(|i| std::cmp::Reverse(i.is_path_default));
    installs
}

/// 工具对应的 npm 包名（hermes 走自己的 CLI/installer，不在此表）。锚定升级据此拼 `npm i -g`。
/// 全平台共用一张表——Windows 锚定层(`anchored_command_from_paths` 的 windows 版)也读这里。
fn npm_package_for(tool: &str) -> Option<&'static str> {
    match tool {
        "claude" => Some("@anthropic-ai/claude-code"),
        "codex" => Some("@openai/codex"),
        "gemini" => Some("@google/gemini-cli"),
        "grok" => Some("@xai-official/grok"),
        "opencode" => Some("opencode-ai"),
        "openclaw" => Some("openclaw"),
        "pi" => Some("@earendil-works/pi-coding-agent"),
        "mcode" => Some("@minimax-ai/code"),
        "kimi" => Some("@moonshot-ai/kimi-code"),
        "omp" => Some("@oh-my-pi/pi-coding-agent"),
        "dsh" => Some("@deepseek-ai/dsh"),
        _ => None,
    }
}

/// 取路径的父目录(纯字符串截断,不碰 fs):`/a/b/npm` → `/a/b`、`C:\a\b\npm.cmd`
/// → `C:\a\b`、混合分隔符 `C:\a/b\npm` → `C:\a/b`。无父目录返回空串。
///
/// 平台无关:`\` 和 `/` 都识别,取两者最右出现位置。`Option<usize>` 的 Ord 让
/// `None < Some(_)`,所以 `rfind('\\').max(rfind('/'))` 自动取存在的那个、两者都
/// 存在时取靠右的——比 `or_else` 优先取一种正确(混合分隔符不会拿错父目录)。
/// 跨平台 fs separator 在两侧均接受,使 macOS/Linux 上的 cargo test 也能跑 Windows
/// 路径用例(`parent_dir_cases::mixed_separators_takes_rightmost`)。空串语义由上游
/// `sibling_bin` 的 `is_empty()` 检查转成 None → 锚定整体退化到静态兜底。
fn parent_dir(p: &str) -> String {
    match p.rfind('\\').max(p.rfind('/')) {
        Some(i) if i > 0 => p[..i].to_string(),
        _ => String::new(),
    }
}

/// 从 canonicalize 后的真身路径提取 Homebrew formula 名：
/// `/opt/homebrew/Cellar/gemini-cli/0.13.0/...` → `Some("gemini-cli")`。
/// 非 Cellar 路径（= 不是 formula，可能是 Homebrew 的 node 装的 npm 全局包）返回 None。
/// 关键区分：formula 即便内部用 node，真身也落在 `Cellar/<formula>/` 下；而 Homebrew
/// npm 全局包落在 `/opt/homebrew/lib/node_modules`（不含 Cellar）。两者升级命令不同。
#[cfg(not(target_os = "windows"))]
fn brew_formula_from_path(real: &str) -> Option<String> {
    brew_token_from_path(real, "Cellar")
}

/// 从 canonicalize 后的真身路径提取 Homebrew cask token：
/// `/opt/homebrew/Caskroom/codex/0.146.0/bin/codex` → `Some("codex")`。
/// 非 Caskroom 路径返回 None。Cask 是预编译二进制，不走 node；若误判成 Homebrew
/// npm 全局包，会用 sibling `npm i -g` 覆盖 `/opt/homebrew/bin/<tool>` 并触发 EEXIST。
#[cfg(not(target_os = "windows"))]
fn brew_cask_from_path(real: &str) -> Option<String> {
    brew_token_from_path(real, "Caskroom")
}

/// Homebrew 安装目录形如 `<prefix>/{Cellar,Caskroom}/<token>/<version>/...`。
/// 取出 `marker` 后的第一段作为 formula 名或 cask token；大小写不敏感，兼容
/// `/usr/local/Caskroom`（Intel）与 `/opt/homebrew/Caskroom`（Apple Silicon）。
#[cfg(not(target_os = "windows"))]
fn brew_token_from_path(real: &str, marker: &str) -> Option<String> {
    let mut segs = real.split('/');
    while let Some(seg) = segs.next() {
        if seg.eq_ignore_ascii_case(marker) {
            return segs.next().filter(|s| !s.is_empty()).map(|s| s.to_string());
        }
    }
    None
}

/// Cellar formula 或 Caskroom cask 都由 Homebrew 拥有，升级必须走 brew，
/// 不能落到 sibling npm。任一命中即视为 brew-managed。
#[cfg(not(target_os = "windows"))]
fn is_brew_managed_path(real: &str) -> bool {
    brew_formula_from_path(real).is_some() || brew_cask_from_path(real).is_some()
}

/// xAI's native installer uses `~/.grok/bin` for its launchers and
/// `~/.grok/downloads/grok-<platform>` for the downloaded binary. The launcher
/// directory also covers the default Windows layout, where the executable is
/// copied instead of symlinked. Checking the real target additionally supports
/// a custom `$GROK_BIN_DIR` on POSIX, whose launcher still points into the
/// standard downloads directory.
fn is_grok_native_install(bin_path: &str, real_target: &str) -> bool {
    [bin_path, real_target].iter().any(|path| {
        let normalized = path.replace('\\', "/").to_ascii_lowercase();
        normalized.contains("/.grok/bin/") || normalized.contains("/.grok/downloads/grok-")
    })
}

fn last_path_segment_lower(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

/// MiniMax Code 官方 installer 的入口布局（见 `mcode_extra_search_paths`）：POSIX 为
/// `<root>/bin/mcode`（读 `<root>/current` 再转发的 sh launcher，不是软链），Windows 的
/// `mcode.cmd` 直接放在 `<root>` 下。显式传参而不是在函数里 `cfg`，两种布局都能在任一
/// 平台上单测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McodeLayout {
    Posix,
    Windows,
}

impl McodeLayout {
    const NATIVE: Self = if cfg!(target_os = "windows") {
        Self::Windows
    } else {
        Self::Posix
    };
}

/// 从入口路径推出安装根目录候选（installer 的 `MCODE_INSTALL_DIR`）。只是候选，是否真
/// 属于 installer 由调用方判定。只有 POSIX 布局才去掉入口所在的那层 `bin`：Windows 的
/// 根目录本身可以叫 `bin`（如 `MCODE_INSTALL_DIR=C:\Users\me\bin`），不能再上跳一级。
fn mcode_install_root_candidate(path: &str, layout: McodeLayout) -> String {
    let dir = parent_dir(path);
    if layout == McodeLayout::Posix && last_path_segment_lower(&dir) == "bin" {
        parent_dir(&dir)
    } else {
        dir
    }
}

/// 若入口位于默认安装目录 `.minimax-code`，返回该目录。纯字符串、不碰 fs（与 POSIX
/// 锚定的纯函数约定一致）；自定义 `MCODE_INSTALL_DIR` 由 `installs_anchored_command`
/// 里的 `mcode_receipt_install_root` 按回执认领，这里只兜底回执缺失的情况。
/// 先看 `bin_path`：Windows 的 `real` 是 canonicalize 出的 `\\?\` verbatim 路径，
/// 不宜原样交给 installer；`real_target` 覆盖用户自建软链指向 launcher 的情况。
fn mcode_script_install_root(
    bin_path: &str,
    real_target: &str,
    layout: McodeLayout,
) -> Option<String> {
    [bin_path, real_target].iter().find_map(|path| {
        let root = mcode_install_root_candidate(path, layout);
        (last_path_segment_lower(&root) == ".minimax-code").then_some(root)
    })
}

/// Kimi Code 的 `upgrade` 子命令 0.43.0 起才有 `-y, --yes`。更早的版本把 `--yes` 当成主命令
/// 的全局选项吞掉，非 TTY（静默 lifecycle 正是如此）下只打印手动升级提示就 exit 0，
/// `kimi upgrade --yes || <fallback>` 的兜底永不触发，用户看到「版本没有变化」（#9032）。
const KIMI_UPGRADE_YES_MIN_VERSION: &str = "0.43.0";

fn kimi_upgrade_ignores_yes(version: Option<&str>) -> bool {
    version.and_then(|v| compare_semver(v, KIMI_UPGRADE_YES_MIN_VERSION))
        == Some(std::cmp::Ordering::Less)
}

/// Kimi Code 官方 installer 的布局：两平台都是 `<KIMI_INSTALL_DIR>/bin/kimi[.exe]`
/// 原生二进制。npm 版的真身在 `node_modules` 下（Windows 入口是 `kimi.cmd`），不算。
/// 先看真身：POSIX 用户自建的软链（如 `~/.local/bin/kimi`）指回安装目录时，installer
/// 必须写回真身所在的那处；Windows 的真身是 canonicalize 出的 `\\?\` verbatim 路径，
/// 交给 installer 前还原。纯字符串、不碰 fs。
fn kimi_native_install_root(bin_path: &str, real_target: &str) -> Option<String> {
    let normalized_real = real_target.replace('\\', "/").to_ascii_lowercase();
    if normalized_real.contains("/node_modules/") {
        return None;
    }
    #[cfg(not(target_os = "windows"))]
    if is_brew_managed_path(real_target) {
        return None;
    }
    let real = match real_target.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => real_target
            .strip_prefix(r"\\?\")
            .unwrap_or(real_target)
            .to_string(),
    };
    let root = [real.as_str(), bin_path].into_iter().find_map(|path| {
        if !matches!(last_path_segment_lower(path).as_str(), "kimi" | "kimi.exe") {
            return None;
        }
        let bin_dir = parent_dir(path);
        if last_path_segment_lower(&bin_dir) != "bin" {
            return None;
        }
        let root = parent_dir(&bin_dir);
        (!root.is_empty()).then_some(root)
    });
    root
}

/// 旧版 Kimi Code（见 `KIMI_UPGRADE_YES_MIN_VERSION`）的升级命令：跳过会假成功的
/// `kimi upgrade --yes`。包管理器认得的安装（npm / Volta / bun / Homebrew）走锚定的包管理器；
/// 先查它，因为 Volta 的 `bin/kimi[.exe]` shim 不解析到 `node_modules`，会被误认成原生
/// 布局、让 installer 覆盖 shim。其余原生安装重跑官方 installer。都认不出时返回 None，
/// 交回普通锚定。
fn kimi_legacy_update_command(bin_path: &str, real_target: &str) -> Option<String> {
    #[cfg(not(target_os = "windows"))]
    let package_command =
        package_manager_anchored_command_from_paths("kimi", bin_path, real_target);
    #[cfg(target_os = "windows")]
    let package_command = package_manager_anchored_command_from_paths("kimi", bin_path);
    package_command.or_else(|| {
        kimi_native_install_root(bin_path, real_target)
            .map(|root| kimi_installer_update_command(&root))
    })
}

/// 按 installer 写在 `<root>/install.json` 的回执认领安装目录——自定义
/// `MCODE_INSTALL_DIR` 的目录名不固定，只能靠它（schema 1/2 都有 `product` /
/// `updateOwner` / `prefix`）。返回回执里的 `prefix` 原值：installer 以
/// `path.resolve(prefix) === path.resolve(MCODE_INSTALL_DIR)` 判定归属，传原值才会
/// 原地升级而非走 repair 迁移。要求 `prefix` 与回执所在目录是同一目录，挡住拷贝来的回执。
fn mcode_receipt_install_root(entry: &str, layout: McodeLayout) -> Option<String> {
    let root = std::path::PathBuf::from(mcode_install_root_candidate(entry, layout));
    let text = std::fs::read_to_string(root.join("install.json")).ok()?;
    // install.ps1 用 `Set-Content -Encoding UTF8` 写回执，Windows PowerShell 5.1 会带
    // UTF-8 BOM；serde_json 不跳过 BOM，不剥掉就在第 1 列报错、退回全局 npm。
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let receipt: serde_json::Value = serde_json::from_str(text).ok()?;
    let field = |key: &str| receipt.get(key).and_then(serde_json::Value::as_str);
    if field("product")? != "minimax-code" || field("updateOwner")? != "npm-prefix" {
        return None;
    }
    let prefix = field("prefix")?;
    let same_dir = std::fs::canonicalize(prefix).ok()? == std::fs::canonicalize(&root).ok()?;
    same_dir.then(|| prefix.to_string())
}

/// 含空格才用 POSIX 单引号包一层,否则保持裸路径——命令展示更干净。
/// claude / brew / volta / bun / npm 五个锚定分支共用,避免"含空格"判定漂移。
///
/// **仅按空格判定,不防其他 shell 元字符**(`$` / `` ` `` / `'` / `"` / `;` 等)。
/// 调用方传入的是探测得到的可执行路径(`enumerate_tool_installations` 里来源于
/// `Path::display()`),实际 macOS/Linux 上 home dir 名几乎不允许这类字符、
/// npm/brew/volta/bun 也不会装到含这类字符的路径,与 diff 前内联在 npm 分支里的
/// `if npm.contains(' ')` 实现等价。若未来要扩广,改成 `shell_single_quote` 无条件
/// 包裹即可,但会失去"无空格时的清洁展示"。
#[cfg(not(target_os = "windows"))]
fn quote_path_if_spaced(p: &str) -> String {
    if p.contains(' ') {
        shell_single_quote(p)
    } else {
        p.to_string()
    }
}

/// 锚定路径走 `.bat` 文件且**被 `call` 调用**,需要为 batch 特殊字符做两层防御:
///
/// **(1) `%` 经历两轮 percent expansion → 用 4 个 `%` 转义**。.bat 中字面 `%` 的
/// 标准转义是 `%%`,但 `call` 命令(Microsoft `call /?`:"percent (%) expansion is
/// performed on each parameter")**在 batch parser 处理完 `%%` → `%` 后自己再做一轮**。
/// 所以源 .bat 里写 `%%FOO%%`,batch 一轮变 `%FOO%`,call 二轮当成 variable reference
/// 又展开一次——要让最终 call 看到字面 `%FOO%` 必须写 `%%%%FOO%%%%`(一轮 → `%%FOO%%`,
/// 二轮 → `%FOO%` 字面)。这是 cmd 唯一**引号无法保护**的字符:引号内的 `%` 仍参与
/// 两轮 expansion。
///
/// **(2) token 边界 / escape 字符触发外层双引号**:`' '` `'&'` `'('` `')'` `'^'`
/// `';'` `'<'` `'>'` `'|'` `','` 任一出现即包引号。NTFS 允许这些字符出现在路径中,
/// 不包会让 cmd 把路径切成多 token、`^` 又会触发 escape;引号内它们是字面意义,
/// 而且 call 二次解析对引号内的它们也不会做特殊处理(`^` 在引号内失去 escape 作用,
/// token 边界字符在引号内是字面)。
///
/// `!`(delayed expansion)只在 `setlocal enabledelayedexpansion` 下生效——我们
/// .bat 头只有 `chcp 65001` 和 `@echo off`、没开,所以不需要处理。`'` 在 cmd 中无特殊意义。
///
/// 镜像 POSIX `quote_path_if_spaced` 的"轻量条件包装"语义:不含任何特殊字符就保持
/// 裸路径(命令展示更干净),否则用 `win_double_quote` 包并做必要转义。
#[cfg(target_os = "windows")]
fn win_quote_path_for_batch(p: &str) -> String {
    // `%` 经历两轮 expansion:.bat parser 一轮 + `call` 二轮(Microsoft `call /?`:
    // "percent (%) expansion is performed on each parameter")。要让 call 最终看到
    // 字面 `%` 需要 4 个 → `%%%%`(batch 一轮 → `%%`,call 二轮 → `%` 字面)。
    // 引号内仍参与两轮 expansion,所以这一步独立于外层引号、必须无条件做。
    let escaped = if p.contains('%') {
        p.replace('%', "%%%%")
    } else {
        p.to_string()
    };
    // 注:`needs_quote` 基于**原路径** `p` 判断,不能用 `escaped`——后者引入的 `%`
    // 字符不算"特殊触发字符",否则含 `%` 的路径会被错误地额外加引号。
    let needs_quote = p
        .chars()
        .any(|c| matches!(c, ' ' | '&' | '(' | ')' | '^' | ';' | '<' | '>' | '|' | ','));
    if needs_quote {
        win_double_quote(&escaped)
    } else {
        escaped
    }
}

/// Windows 版 sibling 推导:在 `<bin_path 父目录>` 下按 `ext_candidates` 顺序找
/// 第一个存在的 `<exe_basename>.<ext>` 文件,返回该绝对路径。
///
/// **与 POSIX `sibling_bin` 的关键区别:这里碰 fs**——Windows 上 npm/pnpm 的入口
/// 实际扩展名可能是 `.cmd` 也可能是 `.exe`(Node.js installer 装的是 `npm.cmd`、
/// 部分 pnpm 是 `pnpm.exe`),纯字符串拼接无法知道哪个真的存在,猜错会拼出
/// "GUI 执行时 file not found" 的命令。fs 检查放进 helper、单测用 tempdir 覆盖,
/// 让上层 `anchored_command_from_paths` 仍保持"接收已锚定路径"的接口形态。
///
/// **TOCTOU 是 by design**:预检 `is_file` 是为了让确认对话框展示真实命令字符串;
/// 检查到执行之间被外部进程(卸载器 / nvm switch / 杀软隔离)移走文件 → cmd /C
/// 报 ENOENT,toast 显示错误。不要在执行前再做二次预检——双重 syscall 也解决不了 race。
///
/// 候选扩展名顺序按工具 idiom:npm/pnpm 优先 `.cmd`(node 装的),volta 优先 `.exe`
/// (Volta 是 Rust 写的 native binary)。
///
/// **不用 `which::which_in` 的理由**:per-tool 扩展名优先级(volta 偏 `.exe`、npm/pnpm
/// 偏 `.cmd`)与 PATHEXT 的固定顺序不一致,而且只为这一处加 `which` 依赖收益不抵 audit
/// surface。`PathBuf::join` 让 separator 选择交给 std,避免 `format!("{dir}\\...")`
/// 硬编码 `\\` 在混合分隔符 bin_path 下产出丑陋路径。
///
/// 空 dir 或所有候选都不存在 → None,上游退化到静态命令,与 POSIX 路径同款语义。
#[cfg(target_os = "windows")]
fn sibling_bin_with_ext(
    bin_path: &str,
    exe_basename: &str,
    ext_candidates: &[&str],
) -> Option<String> {
    let dir = parent_dir(bin_path);
    if dir.is_empty() {
        return None;
    }
    let dir = std::path::PathBuf::from(dir);
    for ext in ext_candidates {
        let candidate = dir.join(format!("{exe_basename}.{ext}"));
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// 返回 `<bin_path 同目录>/<exe>` 的绝对路径。bin_path 是命令行命中的入口
/// (如 `/opt/homebrew/bin/gemini`、`~/.volta/bin/codex`),`exe` 是与之共处一个
/// bin 目录的另一个可执行(`brew` / `volta` / `bun` / `npm`)——这些包管理器
/// 都把自己的 cli 跟它们安装的命令并列放在同一个 bin 目录,所以"同目录推导"
/// 是可靠的绝对路径来源。
///
/// **dir 为空(bin_path 不含 `/`) → 返回 None**:此时无法推导出绝对路径,让上游
/// `anchored_command_from_paths` 整体退化为 None,调用方落到静态命令兜底——而非
/// 悄悄拼出 `npm i -g <pkg>` 这种依赖 PATH 的指令,违背"必须绝对路径"不变量。
/// 实际从 `enumerate_tool_installations` 走的 bin_path 都是 `Path::display()` 出
/// 来的绝对路径,这条防线不期望被触发,但闭合了 helper 与函数文档的语义一致。
#[cfg(not(target_os = "windows"))]
fn sibling_bin(bin_path: &str, exe: &str) -> Option<String> {
    let dir = parent_dir(bin_path);
    if dir.is_empty() {
        None
    } else {
        Some(format!("{dir}/{exe}"))
    }
}

/// Build an npm command anchored to the same Node installation as `bin_path`.
///
/// npm's POSIX launcher is normally a JavaScript file with an
/// `#!/usr/bin/env node` shebang. Calling npm by absolute path therefore does
/// not make it independent of PATH: `env` still needs to find `node`. GUI apps
/// commonly inherit only the system PATH, while nvm/fnm/mise keep Node in a
/// user directory. Prefixing npm's sibling directory makes both npm and its
/// transitive Node interpreter resolve to the installation we selected.
#[cfg(not(target_os = "windows"))]
fn anchored_npm_command(bin_path: &str, args: &str) -> Option<String> {
    let dir = parent_dir(bin_path);
    if dir.is_empty() {
        return None;
    }
    let npm = sibling_bin(bin_path, "npm")?;
    Some(format!(
        "PATH={}:\"$PATH\" {} {args}",
        shell_single_quote(&dir),
        quote_path_if_spaced(&npm)
    ))
}

/// POSIX 官方自升级内部再调 npm 时，用环境变量放行该包的 postinstall 与 optional
/// dependency（含义同 `npm_install_extra_args`），只作用于紧随其后的那条命令。
#[cfg(not(target_os = "windows"))]
fn npm_scripts_env_prefix(pkg: &str) -> String {
    format!("npm_config_allow_scripts={pkg} npm_config_ignore_scripts=false npm_config_include=optional ")
}

#[cfg(not(target_os = "windows"))]
fn anchored_official_update_command(tool: &str, bin_path: &str) -> Option<String> {
    official_update_args(tool).map(|args| format!("{} {args}", quote_path_if_spaced(bin_path)))
}

#[cfg(target_os = "windows")]
fn anchored_official_update_command(tool: &str, bin_path: &str) -> Option<String> {
    official_update_args(tool).map(|args| format!("{} {args}", win_quote_path_for_batch(bin_path)))
}

/// Grok Build 原生安装的升级命令 = `<bin 绝对> update || <官方 installer>`。
///
/// **为什么唯独 native Grok 的 self-update 需要 fallback**（claude native / hermes 都没有）：
/// `grok update` 虽然是自包含 Rust 二进制的子命令，**却把 npm 当成自己的分发管道**——
/// 先 spawn `npm view @xai-official/grok version --json` 查最新版，再 spawn
/// `npm i -g @xai-official/grok@<version>` 安装，由该包的 `postinstall.js` 从平台 optional
/// 依赖里解出二进制、安置成 `~/.grok/bin/grok-<version>` 并 relink `grok`。而 `npm` 自身是
/// `#!/usr/bin/env node` 脚本 → **native 安装也隐式硬依赖 PATH 里同时有 `npm` + `node`**。
/// GUI 进程 PATH 由 launchd 给、`run_tool_lifecycle_silently` 又是非登录 `bash -c`，
/// nvm/homebrew 下的 node+npm 均不可见 → grok 内部 spawn 得到 ENOENT，只向用户抛出
/// 费解的 `Error: No such file or directory (os error 2)`（实测复现）。
///
/// **这是上游换了机制**：0.2.111 及更早版本自更新是直接下载二进制到 `~/.grok/downloads/`
/// （彼时 `is_grok_native_install` 假设的 "native = 不碰 npm" 成立），0.2.112 起改走上述 npm
/// 管道（落点随之从 `downloads/` 变为 `bin/`）。**副作用**：npm 全局包那一处安装是
/// `grok update` 自己装出来的，非用户手动所为，故 native 用户也会被 enumerate 到两处；
/// 两处由同一次 postinstall 同步，版本恒等。
///
/// 这正是 `anchored_command_from_paths` 那条"绝对路径 + 必要时把解释器目录放 PATH 首位"
/// 不变量没覆盖的第三类：**执行体自身既不需要解释器、也已用绝对路径，却在内部再 spawn
/// 第三方 CLI**。`login_shell_path` 的 PATH 注入已让绝大多数机器上的 primary 直接成功；
/// 这条 fallback 覆盖的是"这台机器根本没装 node"——用官方 installer 装的用户完全可能如此。
///
/// **fallback 必须是官方 installer，不能是 `npm i -g`**：后者与 primary **同源**——primary
/// 失败的两种现实原因（本机无 node；npm registry 指向未同步该 tarball 的镜像，见
/// npmmirror dist-tag 事故）都会让 npm fallback 一并失败，`||` 形同虚设。官方 installer
/// 是唯一 node-free 的独立路径（只需 `curl`，在 `/usr/bin`，窄 PATH 下可用），且落点同为
/// `~/.grok/bin`，锚定语义分毫不动 —— 真正的降级冗余要求 fallback 与 primary **失败模式不相关**。
///
/// **这条 fallback 还兼具第三重作用：修复上游锚点（勿在重构时丢掉）**。
/// grok 的更新路径由 `~/.grok/config.toml` 的 `[cli] installer` 决定（`npm` / `internal` /
/// `gh-release`），而官方 install.sh 会**无条件把该字段覆写为 `internal`**（其 awk 段落先插入
/// `installer = "internal"`、再跳过 `[cli]` 段里已有的 `installer`/`channel` 行）。于是：
/// 用户一旦因 install 端的 npm fallback 被切进 npm 模式（postinstall.js 会写 `installer = "npm"`
/// 并在每次 npm 更新时重写，自我巩固），只要 npm 路径出任何问题，这里的 `||` 就会把他拉回
/// node-free 的 internal 模式，并顺带修好 npm 模式漏更新的 `~/.grok/bin/agent` launcher。
/// **实测验证**（2026-07-30，窄 PATH + `installer = "npm"`）：primary 抛 `os error 2` → fallback
/// 接管 → 装上最新版 → config 写回 `internal` → agent 对齐 → `.zshrc` 幂等更新不重复。
/// ⇒ install 端保留 npm fallback 是安全的（它是 x.ai 不可达时的唯一退路，防火墙场景需要），
/// 其副作用由本链自愈；**把这里换成 npm fallback 会同时废掉降级冗余和这条自愈路径**。
#[cfg(not(target_os = "windows"))]
fn grok_native_update_command(update: String) -> String {
    chain_update_commands(
        update,
        GROK_INSTALL_UNIX.to_string(),
        LifecycleCommandShell::Posix,
    )
}

/// Windows 版同上，fallback 换成官方 PowerShell installer。
/// **不走 `chain_update_commands`**：它会给 `||` 右侧加 `call`，而这里的 fallback 是
/// `powershell.exe`（不是 `.cmd`/`.bat`），不需要 `call`——与 `hermes_update_windows_command`
/// 同一理由。
#[cfg(target_os = "windows")]
fn grok_native_update_command(update: String) -> String {
    format!("{update} || {}", grok_install_windows_command())
}

/// 哪些工具的"官方 self-update"优先于包管理器升级（生成 `<tool> update || <pkg-mgr>`）。
///
/// **codex 刻意不在此列**：`codex update` 在 npm 安装上只是裸 `npm install -g
/// @openai/codex`（无 `@latest` / `--include=optional` / 不先卸载），却只检查 exit code、
/// 无条件打印 “Update ran successfully”。当 npm 把平台二进制 optional 依赖
/// `@openai/codex-<triple>` 漏装时它仍 **exit 0 假成功**，使外层 `||` 兜底被短路、损坏被
/// 成功 toast 掩盖（用户报告的 “Missing optional dependency” 即源于此）。因此 codex 一律走
/// npm 锚定升级；真正损坏（`runnable=false`）时由 `installs_anchored_command` 的门控改用
/// `codex_repair_command` 的 uninstall+install 自愈，而非交给 codex 自身的 self-update。
fn prefers_official_update(tool: &str, shell: LifecycleCommandShell) -> bool {
    match shell {
        LifecycleCommandShell::Posix => {
            matches!(tool, "claude" | "opencode" | "openclaw" | "kimi" | "omp")
        }
        LifecycleCommandShell::WindowsBatch => {
            matches!(
                tool,
                // OpenCode 的 Windows `upgrade` 在 anomalyco/opencode#17295 修复前可能因
                // 安装方式探测失败弹交互 prompt（spawn npm.cmd 没传 shell:true）；静默
                // lifecycle 没有 stdin 会挂死，Windows 先锚到包管理器路径，等上游修了
                // 再把 opencode 加回这里。
                "claude" | "openclaw" | "kimi" | "omp"
            )
        }
    }
}

/// Codex 平台分发包损坏的自愈命令。Codex 的 npm 包是「主包 `@openai/codex`（纯 JS
/// launcher）+ 平台二进制 optional 依赖 `@openai/codex-<triple>`」的分发模式（同 esbuild/swc）。
/// 当平台二进制缺失时 codex 跑不起来——`enumerate_tool_installations` 跑 `--version` 会拿到
/// “Missing optional dependency” 的非 0 退出，标记 `runnable=false`。此状态下普通
/// `npm i -g @pkg@latest` 是 **no-op**：npm 视 optional 依赖缺失为非致命，reify 又认为主包已是
/// 最新（外加半损坏留下的空 nested `node_modules` 残骸强化「tree 已满足」判断），不会补回平台
/// 二进制。唯一实测可靠的修复是先 `uninstall` 清掉残骸、再 `install` 装回完整的主包 + 平台二进制
/// （实测输出 `added 2 packages`）。
///
/// 锚定到与 codex 入口同目录的 npm，并把该目录放到每条 npm 命令的 PATH 首位，使 npm
/// 的 `#!/usr/bin/env node` 能找到同一安装下的 node，不依赖 GUI 非登录进程的 PATH。
/// `|| true` 让 uninstall 失败（如 nvm 上对半损坏包静默返回非 0）不触发外层 `set -e`
/// 中止，但随后的 install 若失败仍会被 `set -e` 捕获并上报给前端 toast。
///
/// **仅对会锚定到 sibling npm 的 node 管理器来源（nvm/fnm/mise/homebrew npm）生效**：
/// `runnable=false` 是宽信号（权限 / node 版本 / 任意 `--version` 失败皆可触发），非 npm
/// 全局安装各有自己的二进制分发与修复方式，无脑套 npm uninstall+install 会出错——Homebrew
/// formula / cask（real 在 `Cellar/` 或 `Caskroom/`）本应 `brew upgrade`，npm 够不到它反而旁路装第二份 npm
/// 全局 codex；Volta/Bun 本应 `volta install`/`bun add`，且 `~/.bun/bin` 下没有 npm、
/// `sibling_bin` 会拼出不存在的路径；system/未知来源无可靠 sibling npm。这些来源一律返回
/// None，让上游继续走 source-specific 的 `anchored_command_from_paths`。白名单与
/// `package_manager_anchored_command_from_paths` 的 sibling-npm 分支对齐。
/// 刻意**不**额外用 `inst.error` 文本确认「确系缺二进制」：enumerate 只保留 stderr 末尾 4 行，
/// 而 codex.js 抛错的 "Missing optional dependency" 行会被尾部 node stack `at ...` 行挤出窗口
/// （实测用户原始错误即如此），强加该条件反而漏修真实缺包；对 npm 全局安装，uninstall+install
/// 对各类损坏都是合理且不会更糟的修复。
#[cfg(not(target_os = "windows"))]
fn codex_repair_command(bin_path: &str, real: &str) -> Option<String> {
    // brew formula / cask（real 在 Cellar 或 Caskroom）→ 不归 npm 管，交回 anchored 走 brew upgrade。
    // 官方独立安装同理：`CODEX_INSTALL_DIR` 可以指到 Homebrew / nvm 的 bin 目录，入口来源
    // 会被判成 homebrew / nvm，但真身在 standalone 发布包里，交回 anchored 重跑 installer。
    if is_brew_managed_path(real) || codex_standalone_install(bin_path, real).is_some() {
        return None;
    }
    // 只认会落到 sibling npm 的 node 管理器来源；volta/bun/system/未知交回 anchored。
    if !matches!(
        infer_install_source(Path::new(bin_path)),
        "nvm" | "fnm" | "mise" | "homebrew"
    ) {
        return None;
    }
    let pkg = "@openai/codex";
    let uninstall = anchored_npm_command(bin_path, &format!("uninstall -g {pkg}"))?;
    let install = anchored_npm_command(bin_path, &format!("i -g {pkg}@latest"))?;
    Some(format!("{uninstall} || true; {install}"))
}

/// Windows 暂不做平台分发自愈：Windows 上 codex 的破坏模式不同（EPERM 文件锁 / 版本 bump
/// 残留，见 openai/codex#21872、#19824），且 `.bat` 链的错误处理与 POSIX `set -e` 语义不同，
/// 需要单独设计；先在本问题实际发生的 POSIX 平台落地。返回 None → 上游走正常锚定命令。
#[cfg(target_os = "windows")]
fn codex_repair_command(_bin_path: &str, _real: &str) -> Option<String> {
    None
}

#[cfg(not(target_os = "windows"))]
fn package_manager_anchored_command_from_paths(
    tool: &str,
    bin_path: &str,
    real_target: &str,
) -> Option<String> {
    // Cask 必须先于 formula：两者互斥（真身不会同时落在 Caskroom 和 Cellar），
    // 但先拦 cask 能避免未来路径里同时出现这两个段时误走 formula。
    if let Some(cask) = brew_cask_from_path(real_target) {
        let brew = sibling_bin(bin_path, "brew")?;
        return Some(format!(
            "{} upgrade --cask {cask}",
            quote_path_if_spaced(&brew)
        ));
    }
    if let Some(formula) = brew_formula_from_path(real_target) {
        let brew = sibling_bin(bin_path, "brew")?;
        return Some(format!("{} upgrade {formula}", quote_path_if_spaced(&brew)));
    }
    let pkg = npm_package_for(tool)?;
    let source = infer_install_source(Path::new(bin_path));
    // omp 的 npm 包只能由 bun 运行：除 bun 外的 node 管理器装不出能跑的 omp。
    if tool == "omp" && source != "bun" {
        return None;
    }
    match source {
        "volta" => {
            let volta = sibling_bin(bin_path, "volta")?;
            return Some(format!("{} install {pkg}", quote_path_if_spaced(&volta)));
        }
        "bun" => {
            let bun = sibling_bin(bin_path, "bun")?;
            return Some(format!(
                "{} add -g {pkg}@latest",
                quote_path_if_spaced(&bun)
            ));
        }
        // 自带同级 npm 的 node 管理器：落到下面锚定到那处的 npm。
        "nvm" | "fnm" | "mise" | "homebrew" => {}
        // system / 未知来源通常没有同级 npm，不能拼 `<dir>/npm`。若工具有官方
        // self-update，上层会直接锚到 CLI 自身；否则返回 None 走静态兜底。
        _ => return None,
    }
    anchored_npm_command(
        bin_path,
        &format!("i -g {pkg}@latest{}", npm_install_extra_args(tool)),
    )
}

/// 给定工具、原始 bin 路径（命令行命中的入口）、canonicalize 后的真身路径，
/// 推断"写回同一处"的锚定升级命令。**POSIX 版是纯函数（不碰 FS）**——真实 canonicalize
/// 由调用方做（`installs_anchored_command` 复用 enumerate 时算出的 `inst.real`),
/// 便于单测覆盖各包管理器分支。Windows 版同名函数因 sibling 扩展名歧义必须读 fs,
/// 是刻意保留的平台差异(详见 Windows 版本 doc)。
///
/// **关键不变量：返回的命令必须用绝对路径调用执行体；若执行体通过
/// `#!/usr/bin/env` 查找解释器，还必须把其同级 bin 目录显式放到 PATH 首位**。
/// 这条命令最终在 `run_tool_lifecycle_silently` 的非登录 `bash -c` 里执行——
/// GUI App 启动的进程 PATH 由 launchd / Windows Service / systemd 给,通常**不含**
/// `~/.local/bin` / `/opt/homebrew/bin` / `~/.volta/bin` 等用户级 bin 目录;而探测
/// 阶段 `try_get_version` 用的是 `$SHELL -lic`(登录+交互式,会读 .zshrc/.zprofile),
/// 两者 PATH 不对称。裸 `claude update` / `brew upgrade ...` 在 GUI 进程里大概率
/// `command not found`(exit 127)→ `set -e` 中止 → 用户看到失败 toast,锚定决策却
/// 已展示给用户"将写回原生那处"——欺骗性故障。
///
/// 判定顺序（命中即返回）：
/// ① Hermes → `<bin_path 绝对> update`;Hermes CLI 自己知道安装环境,避免 cc-switch
///    猜系统 `python3`/`python` 时撞上 Python 版本或 pyenv shim 问题。
/// ② Claude / Grok 原生安装器 → `<bin_path 绝对> update`；
///    bin_path 指向 launcher,launcher 内部 dispatch update 子命令。它不归 npm 管,
///    且在 PATH 里比 nvm/homebrew 更靠前,用 npm 升级会装到别处且被原生那份遮蔽。
///    MiniMax Code 的脚本安装同理不归全局 npm 管,但 `mcode update` 非 TTY 下不安装
///    (见 `official_update_args` 上方注释),改为带 `MCODE_INSTALL_DIR` 重跑官方 installer。
///    Codex 独立安装器同理:`codex update` 断网时假成功(见 `CODEX_INSTALL_UNIX`),
///    改为带 `CODEX_INSTALL_DIR` / `CODEX_HOME` 重跑官方 installer。
/// ③ Homebrew formula / cask（真身在 `Cellar/<formula>/` 或 `Caskroom/<token>/`）
///    → `<bin_path 同目录>/brew upgrade [--cask] <name>`；由 Homebrew 拥有,避免
///    self-update 或 sibling npm 改动包管理器管理的安装。
/// ④ 其余支持官方自升级的工具 → `<bin_path 绝对> update/upgrade || <原锚定包管理器命令>`；
///    Codex 的 self-update 只在部分 release 可用,所以保留 npm/brew/bun/volta fallback。
/// ⑤ 不支持官方自升级的 npm 全局包(例如 Gemini CLI，以及非 native 的 Grok Build) → 锚定到
///    "那处 bin 目录的 npm"。
#[cfg(not(target_os = "windows"))]
fn anchored_command_from_paths(tool: &str, bin_path: &str, real_target: &str) -> Option<String> {
    let real_lower = real_target.to_ascii_lowercase();

    // Antigravity 原生二进制自升级（Homebrew cask 也标了 auto_updates，`brew upgrade` 默认跳过它）。
    if tool == "hermes" || tool == "agy" {
        return anchored_official_update_command(tool, bin_path);
    }
    if tool == "claude"
        && (real_lower.contains("/.local/share/claude/")
            || real_lower.contains("/claude/versions/"))
    {
        return anchored_official_update_command(tool, bin_path);
    }
    if tool == "grok" && is_grok_native_install(bin_path, real_target) {
        // `[cli] installer = "npm"`（npm 包的 postinstall 写入）下，`grok update` 会 spawn
        // `npm i -g @xai-official/grok@<version>`，靠 postinstall relink `~/.grok/bin/grok`。
        // npm 12 拦截后仍 exit 0，`||` 后的官方 installer 不会接手，用户继续跑旧版。
        // 环境变量只作用于 `||` 左侧这一条；internal / gh-release 模式不经 npm，不受影响。
        return Some(grok_native_update_command(format!(
            "{}{}",
            npm_scripts_env_prefix("@xai-official/grok"),
            anchored_official_update_command(tool, bin_path)?
        )));
    }
    if tool == "mcode" {
        if let Some(root) = mcode_script_install_root(bin_path, real_target, McodeLayout::Posix) {
            return Some(mcode_installer_update_command(&root));
        }
    }
    if tool == "codex" {
        if let Some(install) = codex_standalone_install(bin_path, real_target) {
            return Some(codex_installer_update_command(&install));
        }
    }
    // Nix 安装 `omp update` 只提示去改 flake 就 exit 0，执行它会报假成功。
    if tool == "omp" && real_lower.starts_with("/nix/store/") {
        return None;
    }
    let package_command = package_manager_anchored_command_from_paths(tool, bin_path, real_target);
    if is_brew_managed_path(real_target) {
        return package_command;
    }
    if prefers_official_update(tool, LifecycleCommandShell::Posix) {
        let mut update = anchored_official_update_command(tool, bin_path)?;
        // npm-installed Claude / OpenCode delegate updates to npm. Keep the npm 12
        // postinstall and optional dependency settings scoped to this process
        // tree; other installation sources retain their own update policy.
        // Gate on the real target rather than the launcher's source so npm
        // prefixes classified as `system` (Intel Homebrew node, nodejs.org pkg,
        // ~/.npm-global, distro node) are covered too: they have no sibling-npm
        // fallback, so a false-success update there could never self-heal.
        // Bun-installed OpenCode upgrades through bun, not npm.
        let npm_scripts_package = match tool {
            "claude" => Some("@anthropic-ai/claude-code"),
            "opencode" if !real_lower.contains("/.bun/") => Some("opencode-ai"),
            _ => None,
        };
        if let Some(pkg) = npm_scripts_package {
            if real_lower.contains(&format!("/node_modules/{pkg}/")) {
                update = format!("{}{update}", npm_scripts_env_prefix(pkg));
            }
        }
        return Some(match package_command {
            Some(fallback) => chain_update_commands(update, fallback, LifecycleCommandShell::Posix),
            None => update,
        });
    }
    package_command
}

#[cfg(target_os = "windows")]
fn package_manager_anchored_command_from_paths(tool: &str, bin_path: &str) -> Option<String> {
    // omp 的 npm 包只能由 bun 运行，这里的 npm / Volta / pnpm 都装不出能跑的 omp。
    if tool == "omp" {
        return None;
    }
    let pkg = npm_package_for(tool)?;

    match infer_install_source(Path::new(bin_path)) {
        "volta" => {
            let volta = sibling_bin_with_ext(bin_path, "volta", &["exe", "cmd"])?;
            Some(format!(
                "{} install {pkg}",
                win_quote_path_for_batch(&volta)
            ))
        }
        "pnpm" => {
            let pnpm = sibling_bin_with_ext(bin_path, "pnpm", &["cmd", "exe"])?;
            Some(format!(
                "{} add -g {pkg}@latest",
                win_quote_path_for_batch(&pnpm)
            ))
        }
        // 兜底 = npm 类:Scoop / Chocolatey / winget / nvm-windows / MS Store nodejs /
        // system / 任何识别不到专属来源的 → sibling npm.cmd。
        _ => {
            let npm = sibling_bin_with_ext(bin_path, "npm", &["cmd", "exe"])?;
            Some(format!(
                "{} i -g {pkg}@latest{}",
                win_quote_path_for_batch(&npm),
                npm_install_extra_args(tool)
            ))
        }
    }
}

/// Windows 版锚定命令生成。对平台确认可静默运行的工具优先使用官方 CLI 自升级；
/// 对 npm/Volta/pnpm 这类可确认写回位置的安装，再接一个包管理器 fallback。不存在 brew/bun/claude-native
/// (Windows 没 Homebrew、Bun for Windows 仍 preview；Grok native 使用 PowerShell installer)。
/// Scoop/Chocolatey/winget/nvm-windows/MS Store node 都归 npm 类——它们都只是"如何装
/// node"的不同入口,全局包真正的 idiom 仍是 sibling `npm.cmd`。
///
/// **与 POSIX 版的语义差异**:POSIX 版是纯函数(不碰 fs),Windows 版通过
/// `sibling_bin_with_ext` 读 fs 来探明扩展名(`.cmd` vs `.exe`)——Node installer
/// 装 `.cmd`、Volta 装 `.exe`,纯字符串拼接无法消歧。这一平台差异**被刻意保留**:
/// 测试用 tempdir 隔离 fs,生产侧 TOCTOU 是 by design(见 `sibling_bin_with_ext` doc)。
///
/// `real_target` 维持与 POSIX 版的签名对称，并辅助识别 Grok native。若未来加 Scoop
/// persist 锚定(scoop 装的工具真身在 `<scoop_root>/persist/<app>/...`),也从这里取真身。
///
/// **关键不变量同 POSIX 版:返回的命令必须用绝对路径,不依赖 PATH**。Windows GUI
/// 进程 PATH 由 Service Control Manager / explorer.exe 给,通常不含用户 `%LOCALAPPDATA%`
/// 下的 Volta/pnpm 路径;`$SHELL -lic` 的探测时 PATH 与执行时 PATH 不对称。
///
/// 判定顺序(命中即返回):
/// ① hermes / Grok native → `<bin_path> update`;CLI 自己处理安装环境。
///    MiniMax Code 脚本安装 → 带 `MCODE_INSTALL_DIR` 重跑官方 PowerShell installer。
///    Codex 独立安装 → 带 `CODEX_INSTALL_DIR` / `CODEX_HOME` 重跑官方 PowerShell installer。
/// ② 支持官方自升级且 Windows 可安全静默执行的工具 → `<bin_path> update/upgrade || call <包管理器 fallback>`。
/// ③ 其余 npm 工具 → sibling `npm.cmd`/`.exe` i -g <pkg>@latest。
///
/// 包管理器 fallback 的 sibling 探测都通过 `sibling_bin_with_ext`(碰 fs):该处无候选
/// 扩展名存在时,支持官方自升级的工具仍返回 `<bin_path> update/upgrade`,其余工具
/// 才返 None 让上游兜回静态命令、`anchored=false`。
#[cfg(target_os = "windows")]
fn anchored_command_from_paths(tool: &str, bin_path: &str, real_target: &str) -> Option<String> {
    // Antigravity 原生二进制自升级（Homebrew cask 也标了 auto_updates，`brew upgrade` 默认跳过它）。
    if tool == "hermes" || tool == "agy" {
        return anchored_official_update_command(tool, bin_path);
    }
    if tool == "grok" && is_grok_native_install(bin_path, real_target) {
        return Some(grok_native_update_command(
            anchored_official_update_command(tool, bin_path)?,
        ));
    }
    if tool == "mcode" {
        if let Some(root) = mcode_script_install_root(bin_path, real_target, McodeLayout::Windows) {
            return Some(mcode_installer_update_command(&root));
        }
    }
    if tool == "codex" {
        if let Some(install) = codex_standalone_install(bin_path, real_target) {
            return Some(codex_installer_update_command(&install));
        }
    }
    let package_command = package_manager_anchored_command_from_paths(tool, bin_path);
    if prefers_official_update(tool, LifecycleCommandShell::WindowsBatch) {
        let update = anchored_official_update_command(tool, bin_path)?;
        return Some(match package_command {
            Some(fallback) => {
                chain_update_commands(update, fallback, LifecycleCommandShell::WindowsBatch)
            }
            None => update,
        });
    }
    package_command
}

/// 从枚举结果里取"命令行实际命中的那处"：优先 `is_path_default`；否则（解析不到
/// PATH 默认、但只有一处）取唯一那处；多处且无默认标记 → None（无从锚定）。
///
/// 全平台共用——POSIX 和 Windows 版的 `anchored_command_from_paths` 都通过
/// `installs_anchored_command` 调它,取默认那处再 canonicalize 拿真身。
fn default_install(installs: &[ToolInstallation]) -> Option<&ToolInstallation> {
    installs.iter().find(|i| i.is_path_default).or_else(|| {
        if installs.len() == 1 {
            installs.first()
        } else {
            None
        }
    })
}


#[derive(Clone, Copy)]
pub(crate) struct CommandDeadline {
    expires_at: std::time::Instant,
    limit: std::time::Duration,
}

impl CommandDeadline {
    pub(crate) fn from_timeout(timeout: Option<std::time::Duration>) -> Option<Self> {
        timeout.map(|limit| Self {
            expires_at: std::time::Instant::now() + limit,
            limit,
        })
    }

    fn remaining(self) -> Result<std::time::Duration, String> {
        self.expires_at
            .checked_duration_since(std::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| self.timeout_error())
    }

    fn timeout_error(self) -> String {
        format!("Command timed out after {}s", self.limit.as_secs())
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn terminate_child_tree(child: &mut std::process::Child) -> bool {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    let status = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(status, Ok(status) if status.success()) || child.kill().is_ok()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn terminate_child_tree(child: &mut std::process::Child) -> bool {
    let process_group = -(child.id() as libc::pid_t);
    // SAFETY: runtime commands are placed in a dedicated process group before spawn.
    (unsafe { libc::kill(process_group, libc::SIGKILL) == 0 }) || child.kill().is_ok()
}

#[cfg(not(target_os = "windows"))]
fn isolate_child_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;

    // setsid 而非 process_group(0)：新会话自带新进程组（组长=自身，
    // terminate_child_tree 的 kill(-pid) 整组击杀语义不变），并额外**脱离控制终端**。
    // 只隔离进程组时，探测用的交互式 shell（zsh -lic）若还持有控制终端（如 dev 模式
    // 从终端启动），其作业控制会因处于背景进程组被 SIGTTIN/SIGTTOU 停住，`wait()`
    // 永远等不到退出；脱离终端后 shell 拿不到 /dev/tty，作业控制自动关闭。
    // SAFETY: setsid 是 async-signal-safe；fork 出的子进程继承父进程组、必不是组长，
    // 调用不会因 EPERM 失败。
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn wait_child_output(
    mut child: std::process::Child,
    deadline: Option<CommandDeadline>,
) -> Result<std::process::Output, String> {
    use std::io::Read;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_handle = stdout_pipe.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_handle = stderr_pipe.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    });

    let status = match deadline {
        None => child
            .wait()
            .map_err(|e| format!("Failed to wait for command: {e}"))?,
        Some(deadline) => {
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => {
                        let remaining = match deadline.remaining() {
                            Ok(remaining) => remaining,
                            Err(error) => {
                                if terminate_child_tree(&mut child) {
                                    let _ = child.wait();
                                }
                                // Do not join pipe readers on timeout. If tree termination fails,
                                // a descendant may still own the write handle and never produce EOF.
                                drop(stdout_handle);
                                drop(stderr_handle);
                                return Err(error);
                            }
                        };
                        std::thread::sleep(std::cmp::min(
                            std::time::Duration::from_millis(50),
                            remaining,
                        ));
                    }
                    Err(e) => {
                        if terminate_child_tree(&mut child) {
                            let _ = child.wait();
                        }
                        return Err(format!("Failed to wait for command: {e}"));
                    }
                }
            }
        }
    };

    if let Some(deadline) = deadline {
        while stdout_handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
            || stderr_handle
                .as_ref()
                .is_some_and(|handle| !handle.is_finished())
        {
            let remaining = match deadline.remaining() {
                Ok(remaining) => remaining,
                Err(error) => {
                    let _ = terminate_child_tree(&mut child);
                    drop(stdout_handle);
                    drop(stderr_handle);
                    return Err(error);
                }
            };
            std::thread::sleep(std::cmp::min(
                std::time::Duration::from_millis(50),
                remaining,
            ));
        }
    }

    let stdout = stdout_handle
        .map(|handle| handle.join().unwrap_or_default())
        .unwrap_or_default();
    let stderr = stderr_handle
        .map(|handle| handle.join().unwrap_or_default())
        .unwrap_or_default();

    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}



#[cfg(target_os = "windows")]
fn apply_extra_env(cmd: &mut std::process::Command, extra_env: &[(&str, String)]) {
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
}

#[cfg(target_os = "windows")]
fn run_windows_tool_command_capture(
    tool_path: &Path,
    args: &[&str],
    new_path: &str,
    deadline: Option<CommandDeadline>,
    extra_env: &[(&str, String)],
    working_dir: &Path,
) -> Result<std::process::Output, String> {
    use std::process::Stdio;

    let mut cmd = build_windows_tool_command(tool_path, args, new_path);
    apply_extra_env(&mut cmd, extra_env);
    cmd.current_dir(working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to run tool: {e}"))?;
    wait_child_output(child, deadline)
}

/// Convert `\\wsl$\Distro\home\user\...` / `\\wsl.localhost\...` to a Linux path.
#[cfg(target_os = "windows")]
fn wsl_unc_path_to_linux(path: &Path) -> Option<String> {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    match prefix.kind() {
        Prefix::UNC(server, _share) | Prefix::VerbatimUNC(server, _share) => {
            let server_name = server.to_string_lossy();
            if !(server_name.eq_ignore_ascii_case("wsl$")
                || server_name.eq_ignore_ascii_case("wsl.localhost"))
            {
                return None;
            }
        }
        _ => return None,
    }

    let mut linux = String::new();
    for component in components {
        match component {
            Component::RootDir => {}
            Component::Normal(part) => {
                linux.push('/');
                linux.push_str(&part.to_string_lossy());
            }
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    if linux.is_empty() {
        None
    } else {
        Some(linux)
    }
}

#[cfg(target_os = "windows")]
fn build_wsl_env_argv(extra_env: &[(&str, String)]) -> Result<Vec<String>, String> {
    let mut env_argv = Vec::new();
    for (key, value) in extra_env {
        if key.is_empty()
            || key.contains('=')
            || key.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(format!("invalid env for {key}"));
        }

        let linux_value = if *key == "OPENCODE_CONFIG_DIR" {
            let Some(value) = wsl_unc_path_to_linux(Path::new(value)) else {
                continue;
            };
            value
        } else {
            value.clone()
        };
        if linux_value.chars().any(char::is_control) {
            return Err(format!("invalid env for {key}"));
        }
        env_argv.push(format!("{key}={linux_value}"));
    }
    Ok(env_argv)
}

#[cfg(target_os = "windows")]
fn build_wsl_tool_command(
    tool: &str,
    args: &[&str],
    deadline: Option<CommandDeadline>,
) -> Result<String, String> {
    let invocation = std::iter::once(tool)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    let command = format!(
        "for flag in -lic -lc -c; do if \"${{SHELL:-sh}}\" \"$flag\" 'command -v {tool}' >/dev/null 2>&1; then exec \"${{SHELL:-sh}}\" \"$flag\" '{invocation}'; fi; done; exit 127"
    );

    let Some(deadline) = deadline else {
        return Ok(command);
    };
    let remaining = deadline.remaining()?;
    let timeout_arg = format!("{:.3}s", remaining.as_secs_f64());
    Ok(format!(
        "command -v timeout >/dev/null 2>&1 || {{ echo 'timeout is required for bounded CLI execution' >&2; exit 127; }}; exec timeout --signal=TERM --kill-after=1s {timeout_arg} sh -c {}",
        shell_single_quote(&command)
    ))
}

#[cfg(target_os = "windows")]
fn run_wsl_tool_command(
    tool: &str,
    args: &[&str],
    distro: &str,
    deadline: Option<CommandDeadline>,
    extra_env: &[(&str, String)],
    working_dir: &Path,
) -> Result<std::process::Output, String> {
    use std::process::{Command, Stdio};

    if !is_valid_wsl_distro_name(distro) {
        return Err(format!("[WSL:{distro}] invalid distro name"));
    }

    let command = build_wsl_tool_command(tool, args, deadline)?;
    let linux_working_dir = wsl_unc_path_to_linux(working_dir)
        .ok_or_else(|| format!("[WSL:{distro}] invalid working directory"))?;
    let env_argv = build_wsl_env_argv(extra_env).map_err(|e| format!("[WSL:{distro}] {e}"))?;

    let mut cmd = Command::new("wsl.exe");
    cmd.arg("-d")
        .arg(distro)
        .arg("--cd")
        .arg(linux_working_dir)
        .arg("--");
    if !env_argv.is_empty() {
        cmd.arg("env");
        for item in &env_argv {
            cmd.arg(item);
        }
    }
    cmd.args(["sh", "-c", &command])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("[WSL:{distro}] failed to run {tool}: {e}"))?;
    let output = wait_child_output(child, deadline).map_err(|e| {
        if e.starts_with("Command timed out") {
            format!("[WSL:{distro}] {e}")
        } else {
            e
        }
    })?;
    if output.status.code() == Some(124) {
        return Err(format!(
            "[WSL:{distro}] {}",
            deadline
                .map(CommandDeadline::timeout_error)
                .unwrap_or_else(|| "Command timed out".to_string())
        ));
    }
    Ok(output)
}

/// 基于已枚举的安装列表生成锚定升级命令（复用 enumerate 结果，避免二次探测）。
/// 读取 enumerate 时已 canonicalize 写入的 `inst.real`,**不再二次 canonicalize**——
/// 既消除冗余 syscall,也闭合"enumerate 与 anchor 看到同一真身"的一致性边界
/// (两次 canonicalize 之间 symlink 被换会让锚定指向不同真身)。
///
/// 全平台共用——`anchored_command_from_paths` 自身是 cfg 二选一(POSIX 五分支 /
/// Windows 三分支),这里只负责取默认那处 + 转发。
fn installs_anchored_command(tool: &str, installs: &[ToolInstallation]) -> Option<String> {
    let inst = default_install(installs)?;
    let real = inst.real.to_string_lossy();
    // Codex 平台分发包损坏自愈：主包在但平台二进制缺失时 codex 跑不起来
    // （runnable=false），此时正常锚定的 `npm i -g @latest` 是 no-op 修不好——改用
    // uninstall+install 重装补回平台二进制。**但仅限会锚定到 sibling npm 的 node 管理器
    // 来源**（codex_repair_command 内按 source/real 收窄，brew/volta/bun/system 交回下方
    // source-specific 锚定，避免误用 npm 重装）。runnable=true 的正常升级也走下方普通锚定
    // 路径（且因 codex 不在 prefers_official_update，不会再跑会假成功掩盖损坏的 `codex update`）。
    // 只认 `--version` 真的退出报错：探测超时 / 启动失败的那处可能是好的，先卸载再装
    // 一旦装失败就把旧版丢了；普通 `npm i -g` 失败时 npm 会回滚、旧版还在。
    if tool == "codex" && !inst.runnable && inst.exited_with_error {
        if let Some(cmd) = codex_repair_command(&inst.path, &real) {
            return Some(cmd);
        }
    }
    // MiniMax Code 脚本安装按回执认领（读 fs，故不放进纯函数 `anchored_command_from_paths`）。
    // 否则自定义 `MCODE_INSTALL_DIR` 会退到全局 `npm i -g`：单处安装不弹确认，静默升级了
    // 别处，命令行实际用的那处版本不变。
    if tool == "mcode" {
        if let Some(root) = [inst.path.as_str(), real.as_ref()]
            .into_iter()
            .find_map(|entry| mcode_receipt_install_root(entry, McodeLayout::NATIVE))
        {
            return Some(mcode_installer_update_command(&root));
        }
    }
    // 版本号来自 `--version` 探测，所以放在这里而不是纯路径的 `anchored_command_from_paths`。
    if tool == "kimi" && kimi_upgrade_ignores_yes(inst.version.as_deref()) {
        if let Some(cmd) = kimi_legacy_update_command(&inst.path, &real) {
            return Some(cmd);
        }
    }
    anchored_command_from_paths(tool, &inst.path, &real)
}

/// 静态命令（= 平台可安全静默执行的官方 CLI 自升级 || `npm i -g <pkg>@latest` /
/// 官方 installer）。锚定探不到默认安装时回退到它；npm fallback 仍等同于
/// "装到 PATH 第一个 npm"的旧行为。
fn static_fallback_command_for(tool: &str, action: ToolLifecycleAction) -> String {
    tool_action_shell_command(tool, action).unwrap_or_default()
}

fn static_fallback_command(tool: &str) -> String {
    static_fallback_command_for(tool, ToolLifecycleAction::Update)
}

/// 升级命令的解析结果。plan（前端确认 / 展示）与 execute（真正执行）共用
/// `resolve_update_command`，保证两边判定一致。
#[derive(Debug, PartialEq, Eq)]
enum UpdateCommand {
    /// 锚定到命令行实际命中的那处。
    Anchored(String),
    /// 定位不到默认那处，或那处认不出渠道但可能归 npm 管（asdf/nodenv shim 等脚本）
    /// → 静态命令，保持旧行为。
    Static(String),
    /// 默认那处是原生可执行文件且不在 `node_modules` 里：不归 npm 管，也认不出是哪个
    /// 安装器装的（winget、Scoop、Nix、手动下载的二进制……）。退回裸 `npm i -g` 只会
    /// 另装一份 npm 版——要么被原生那份遮蔽、版本号不动，要么反过来顶替它（#7650），
    /// 所以不执行，由前端提示用户用原来的安装方式升级。
    Unmanaged,
}

fn resolve_update_command(tool: &str, installs: &[ToolInstallation]) -> UpdateCommand {
    if let Some(command) = installs_anchored_command(tool, installs) {
        return UpdateCommand::Anchored(command);
    }
    if default_install(installs).is_some_and(is_unmanaged_native_install) {
        return UpdateCommand::Unmanaged;
    }
    UpdateCommand::Static(static_fallback_command(tool))
}

/// 只认「原生可执行文件 + 真身不在 `node_modules` 里」。npm 全局包的真身都在
/// `node_modules/<pkg>/` 下（多为 JS 入口，个别包把原生二进制放在包内）；asdf / nodenv
/// 的 shim、Windows npm 的 `.cmd` 是文本脚本，都不算——它们背后可能正是 npm 全局包，
/// 拒绝会误伤，保持旧的静态命令。
fn is_unmanaged_native_install(inst: &ToolInstallation) -> bool {
    let real = inst
        .real
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    !real.contains("/node_modules/") && is_native_executable(&inst.real)
}

/// 按文件头识别原生可执行文件：ELF、Mach-O（单架构 / universal）、Windows PE。
fn is_native_executable(path: &Path) -> bool {
    use std::io::Read;

    let mut magic = [0u8; 4];
    let read = std::fs::File::open(path).and_then(|mut file| file.read_exact(&mut magic));
    if read.is_err() {
        return false;
    }
    matches!(
        magic,
        [0x7f, b'E', b'L', b'F']
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xfe, 0xed, 0xfa, 0xce]
            | [0xca, 0xfe, 0xba, 0xbe]
    ) || magic.starts_with(b"MZ")
}

/// 执行路径的兜底报错。正常流程里前端已按 probe 的 `unmanaged` 跳过该工具并给出
/// 本地化提示；只有 probe 失败、前端直接执行时才会走到这里。
fn unmanaged_update_error(tool: &str, installs: &[ToolInstallation]) -> String {
    let path = default_install(installs)
        .map(|inst| inst.path.as_str())
        .unwrap_or_default();
    format!(
        "{} at {path} was not installed by npm or a recognized installer; update it the way it was installed",
        tool_display_name(tool)
    )
}

/// 新装(install)的命令:对有官方 installer 的工具走「上游推荐 || npm 兜底」短路链,
/// 其余工具透传到 install 静态命令。update fallback 会在平台可安全静默执行时
/// 优先跑官方 CLI 自升级,但 install 端不能先跑 `tool update`,
/// 否则“未安装时安装”的路径会多一次无效失败。
///
/// 设计理由:
/// - install 没有锚点可言(从无到有),但**有"上游推荐方式"这一事实** ——
///   Anthropic、xAI 和 SST(OpenCode)都已将自家 native installer 列为首推、把 npm 列为替代方式。
///   把这层认知补进来,让 install 表与 update 端的锚定决策树共用同一份"上游事实"。
/// - Hermes 使用官方 installer,避免用系统 Python/pip 安装时踩 Python >=3.11 与 pyenv
///   `python` shim 问题;更新路径若能锚定已安装 CLI,则走 `<hermes> update`。
///   **Hermes 没有 npm 包,install 端不享受 `||` 降级**——上游 installer 不可达就只能等。
/// - 对**有 npm 包**的工具(claude/grok/opencode/mcode),短路链(POSIX `||`)保证官方脚本不可达/
///   防火墙拦截时仍能装上,降级到裸 `npm i -g`。官方脚本本身不用 pipe,
///   所以这条路径在 WSL 的 `sh -c` 子 shell 中也不依赖外层 `pipefail`。
/// - Windows 上 Claude/OpenCode 原生不启用（对应 installer 都是 bash 脚本）；Grok
///   与 MiniMax Code 使用官方 PowerShell installer，并同样保留 npm fallback。WSL 作为 Linux 环境
///   复用这套 POSIX 安装优先级。
fn installer_with_npm_fallback(installer: &str, tool: &str) -> String {
    match npm_install_command_for(tool) {
        Some(npm) => chain_update_commands(
            installer.to_string(),
            npm.to_string(),
            LifecycleCommandShell::Posix,
        ),
        None => installer.to_string(),
    }
}

fn posix_install_command_for(tool: &str) -> String {
    match tool {
        "claude" => installer_with_npm_fallback(CLAUDE_INSTALL_UNIX, tool),
        // Grok 的 npm fallback **会切换用户的分发模式**（该包 postinstall 把
        // `~/.grok/config.toml` 的 `[cli] installer` 写成 `npm`，此后 `grok update` 一律走
        // npm、隐式依赖 node）。仍然保留它：官方 installer 不可达（防火墙 / x.ai 被拦）时
        // 这是唯一退路，而副作用可自愈——`grok_native_update_command` 的 `||` 官方 installer
        // 会在 npm 路径出问题时把 `installer` 覆写回 `internal`（见该函数 doc 的实测记录）。
        "grok" => installer_with_npm_fallback(GROK_INSTALL_UNIX, tool),
        "opencode" => installer_with_npm_fallback(OPENCODE_INSTALL_UNIX, tool),
        "mcode" => installer_with_npm_fallback(MCODE_INSTALL_UNIX, tool),
        "hermes" => HERMES_INSTALL_UNIX.to_string(),
        "kimi" => KIMI_INSTALL_UNIX.to_string(),
        "omp" => OMP_INSTALL_UNIX.to_string(),
        "agy" => AGY_INSTALL_UNIX.to_string(),
        _ => static_fallback_command_for(tool, ToolLifecycleAction::Install),
    }
}

#[cfg(not(target_os = "windows"))]
fn install_command_for(tool: &str) -> String {
    posix_install_command_for(tool)
}

/// 计算某工具的升级命令与"是否需确认"。全平台共用一份:
/// - **Windows + WSL 工具**(override 是 `\\wsl$\<distro>\...` UNC 路径)的升级规划
///   始终走 POSIX 静态命令、不锚定:锚定命令是 Windows 主机绝对路径,跨 `wsl.exe`
///   边界进入 distro 文件系统后完全无效;且 `enumerate_tool_installations` 不参与
///   WSL 文件系统、锚定无锚点。这一类显式短路到 `(unix_static, false, false)`,
///   前端不会弹确认。
///   **必须用 `wsl_tool_action_shell_command`(unix 版)而非 `static_fallback_command`**
///   ——后者读 `tool_action_shell_command`,Windows target 给 hermes 返回 PowerShell
///   installer,跨 wsl.exe 后不适用;`build_tool_action_line` 的 WSL 分支也用同一 wrapper,
///   保证 plan 展示给前端的命令与实际执行落 .bat 的命令一致。
/// - 其他平台与 Windows 原生工具走 `resolve_update_command`:命中 → 锚定;
///   None(无默认 / sibling 不存在等)→ 静态兜底、`anchored=false`,
///   前端据此给"默认入口无法确定"诚实文案;默认那处是认不出渠道的原生可执行文件
///   → `Unmanaged`,前端跳过并提示用原安装方式升级。
fn plan_command_for(tool: &str, installs: &[ToolInstallation]) -> (UpdateCommand, bool) {
    #[cfg(target_os = "windows")]
    {
        if wsl_distro_for_tool(tool).is_some() {
            let cmd = wsl_tool_action_shell_command(tool, ToolLifecycleAction::Update)
                .unwrap_or_default();
            return (UpdateCommand::Static(cmd), false);
        }
    }
    (resolve_update_command(tool, installs), installs.len() >= 2)
}

/// 多处安装是否构成"真冲突"：≥2 处，且(版本分歧 或 有的能跑有的跑不起来)。
/// 同版本装两份且都能跑不算冲突（不打扰用户）。诊断展示据此判定。
fn is_conflicting(installs: &[ToolInstallation]) -> bool {
    if installs.len() < 2 {
        return false;
    }
    let distinct_versions: std::collections::HashSet<&Option<String>> =
        installs.iter().map(|i| &i.version).collect();
    let runnable_mixed =
        installs.iter().any(|i| i.runnable) && installs.iter().any(|i| !i.runnable);
    distinct_versions.len() > 1 || runnable_mixed
}

/// 一次"探测工具安装分布"的结果：枚举到的所有安装 + 各项衍生判定。同时服务两条
/// 路径——诊断展示（`is_conflict`）与升级确认（`needs_confirmation`/`command`/`anchored`）。
/// 字段保持 snake_case（与 `ToolInstallation` 一致），前端按同名读取。
#[derive(Debug, serde::Serialize)]
pub struct ToolInstallationReport {
    tool: String,
    /// 该工具枚举到的所有安装。
    installs: Vec<ToolInstallation>,
    /// 严阈值：≥2 且(版本分歧或运行态混合)。诊断按钮/自动补诊据此展示冲突。
    is_conflict: bool,
    /// 宽阈值：≥2 处。升级确认据此弹窗（升级只动一处，任何多处都该让用户知情）。
    needs_confirmation: bool,
    /// 锚定后将执行的升级命令（仅展示；真正执行时后端会重新生成，不信任前端回传）。
    command: String,
    /// 是否成功锚定到某处具体安装。false = 退到裸 fallback 命令（无法确定命令行实际
    /// 命中哪处，或该处无同级 npm）；前端据此给出"默认入口无法确定"的诚实文案。
    anchored: bool,
    /// 默认那处是认不出安装渠道的原生可执行文件（见 `UpdateCommand::Unmanaged`）：
    /// 不会执行升级，此时 `command` 为空；前端跳过该工具并提示用原安装方式升级。
    unmanaged: bool,
}

/// 探测各工具的安装分布：枚举所有安装、标记冲突、生成锚定升级命令。只读、无副作用。
/// 诊断按钮、升级前确认、升级后补诊共用此命令，各取所需字段——避免对同一份枚举结果
/// 散落多套下游判定。
#[tauri::command]
pub async fn probe_tool_installations(
    tools: Vec<String>,
) -> Result<Vec<ToolInstallationReport>, String> {
    let requested = normalize_requested_tools(&tools);
    if requested.is_empty() {
        return Err("No supported tools selected".to_string());
    }
    tokio::task::spawn_blocking(move || {
        requested
            .into_iter()
            .map(|tool| {
                let installs = enumerate_tool_installations(tool);
                let (update, needs_confirmation) = plan_command_for(tool, &installs);
                let (command, anchored, unmanaged) = match update {
                    UpdateCommand::Anchored(command) => (command, true, false),
                    UpdateCommand::Static(command) => (command, false, false),
                    UpdateCommand::Unmanaged => (String::new(), false, true),
                };
                let is_conflict = is_conflicting(&installs);
                ToolInstallationReport {
                    tool: tool.to_string(),
                    installs,
                    is_conflict,
                    needs_confirmation,
                    command,
                    anchored,
                    unmanaged,
                }
            })
            .collect()
    })
    .await
    .map_err(|e| format!("probe task join error: {e}"))
}

/// 「应用」页显示每个工具的路径、安装来源和多处安装。和升级前的预检是同一份枚举，
/// 单列一个命令只为把「打开页面时的展示」和「点升级时的预检」分开调用。
#[tauri::command]
pub async fn list_tool_installations(
    tools: Vec<String>,
) -> Result<Vec<ToolInstallationReport>, String> {
    probe_tool_installations(tools).await
}


#[cfg(target_os = "windows")]
fn wsl_distro_for_tool(tool: &str) -> Option<String> {
    // 通用 API 代理版只保留 Claude 的配置目录覆盖；其余工具没有 override 目录，
    // 一律按默认安装路径探测。
    let override_dir = match tool {
        "claude" => crate::settings::get_claude_override_dir(),
        _ => None,
    }?;

    wsl_distro_from_path(&override_dir)
}


/// 从 UNC 路径中提取 WSL 发行版名称
/// 支持 `\\wsl$\Ubuntu\...` 和 `\\wsl.localhost\Ubuntu\...` 两种格式
#[cfg(target_os = "windows")]
fn wsl_distro_from_path(path: &Path) -> Option<String> {
    use std::path::{Component, Prefix};
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return None;
    };
    match prefix.kind() {
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            let server_name = server.to_string_lossy();
            if server_name.eq_ignore_ascii_case("wsl$")
                || server_name.eq_ignore_ascii_case("wsl.localhost")
            {
                let distro = share.to_string_lossy().to_string();
                if !distro.is_empty() {
                    return Some(distro);
                }
            }
            None
        }
        _ => None,
    }
}

/// 打开指定提供商的终端
///
/// 根据提供商配置的环境变量启动一个带有该提供商特定设置的终端
/// 无需检查是否为当前激活的提供商，任何提供商都可以打开终端
#[allow(non_snake_case)]
#[tauri::command]
pub async fn open_provider_terminal(
    state: State<'_, crate::store::AppState>,
    app: String,
    #[allow(non_snake_case)] providerId: String,
    cwd: Option<String>,
    #[allow(non_snake_case)] skipPermissions: Option<bool>,
    #[allow(non_snake_case)] extraArgs: Option<String>,
) -> Result<bool, String> {
    let app_type = AppType::from_str(&app).map_err(|e| e.to_string())?;
    let launch_cwd = resolve_launch_cwd(cwd)?;
    // 先拆参数：填错了直接报错，不必等到切完 agy 登录、写完临时文件。
    let extra_args = split_launch_args(extraArgs.as_deref().unwrap_or_default())?;

    // 获取提供商配置
    let providers = ProviderService::list(state.inner(), app_type.clone())
        .map_err(|e| format!("获取提供商列表失败: {e}"))?;

    let provider = providers
        .get(&providerId)
        .ok_or_else(|| format!("提供商 {providerId} 不存在"))?;

    let mut launch = match app_type {
        AppType::Claude => {
            let env_vars = extract_env_vars_from_config(&provider.settings_config);
            claude_terminal_launch(&env_vars, &providerId, &provider.name)?
        }
    };
    if skipPermissions == Some(true) {
        launch.skip_permissions();
    }
    if let Err(e) = launch.append_user_args(extra_args) {
        for file in &launch.cleanup {
            let _ = std::fs::remove_file(file);
        }
        return Err(e);
    }

    launch_provider_terminal(&launch, launch_cwd.as_deref())
        .map_err(|e| format!("启动终端失败: {e}"))?;

    Ok(true)
}

/// 从提供商配置中提取 Claude 终端所需的环境变量
fn extract_env_vars_from_config(config: &serde_json::Value) -> Vec<(String, String)> {
    let mut env_vars = Vec::new();

    let Some(obj) = config.as_object() else {
        return env_vars;
    };

    if let Some(env) = obj.get("env").and_then(|v| v.as_object()) {
        for (key, value) in env {
            if let Some(str_val) = value.as_str() {
                env_vars.push((key.clone(), str_val.to_string()));
            }
        }

        if let Some(url_str) = env
            .get("ANTHROPIC_BASE_URL")
            .and_then(|v| v.as_str())
        {
            env_vars.push(("ANTHROPIC_BASE_URL".to_string(), url_str.to_string()));
        }
    }

    env_vars
}

fn resolve_launch_cwd(cwd: Option<String>) -> Result<Option<PathBuf>, String> {
    let Some(raw_path) = cwd.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };

    if raw_path.contains('\n') || raw_path.contains('\r') {
        return Err("目录路径包含非法换行符".to_string());
    }

    let path = Path::new(&raw_path);
    if !path.exists() {
        return Err(format!("目录不存在: {raw_path}"));
    }

    let resolved = std::fs::canonicalize(path).map_err(|e| format!("解析目录失败: {e}"))?;
    if !resolved.is_dir() {
        return Err(format!("选择的路径不是文件夹: {}", resolved.display()));
    }

    #[cfg(target_os = "windows")]
    let resolved = windows_shell_compatible_path(&resolved);

    Ok(Some(resolved))
}

/// 在终端里启动的一次供应商会话：跑什么命令、只给它哪些环境变量、退出后删哪些临时
/// 文件。打开终端那一层（各家终端的 AppleScript、Linux 候选列表、Windows start）对所有
/// 应用共用，只有这里按应用不同。
struct ProviderLaunch {
    /// 启动前打印的说明
    title: &'static str,
    /// 说明下面打印的路径（只在 macOS / Linux 打印）
    #[cfg_attr(windows, allow(dead_code))]
    detail: Option<PathBuf>,
    program: &'static str,
    /// macOS / Linux 的参数，交给用户 shell 执行前逐个加引号
    #[cfg_attr(windows, allow(dead_code))]
    args: Vec<String>,
    /// Windows .bat 里执行的命令
    #[cfg_attr(not(windows), allow(dead_code))]
    batch_command: String,
    /// 只给这一个进程的环境变量；`None` 表示要清掉（用户 rc 里可能 export 过同名变量）。
    /// macOS / Linux 写进 0600 的临时文件、读完立即删除；Windows 写进 .bat 的 `setlocal` 段。
    env: Vec<(&'static str, Option<String>)>,
    /// 只在 .bat 里设置、给 `batch_command` 引用的变量（比如可能含非 ASCII 的路径）
    #[cfg_attr(not(windows), allow(dead_code))]
    batch_vars: Vec<(&'static str, String)>,
    /// 退出时删除的临时文件，都在系统临时目录里
    cleanup: Vec<PathBuf>,
}

impl ProviderLaunch {
    /// 用户在「打开终端」里勾了跳过权限确认：只加给这一次启动，不写进任何配置。
    fn skip_permissions(&mut self) {
        // Claude Code 和 agy 的参数同名；Codex 跳过确认的同时也关掉沙箱。
        let flag = match self.program {
            "codex" => "--dangerously-bypass-approvals-and-sandbox",
            _ => "--dangerously-skip-permissions",
        };
        self.args.push(flag.to_string());
        self.batch_command.push(' ');
        self.batch_command.push_str(flag);
    }

    /// 用户在「打开终端」里填的附加参数，接在 CC Switch 生成的参数后面，同样只给这一次。
    /// Windows 上每个参数经 .bat 变量传、用引号包住，cmd 的 `&`、`|` 之类不会接出别的命令。
    fn append_user_args(&mut self, args: Vec<String>) -> Result<(), String> {
        // Codex 的启动行带 `call`：`%` 会再展开一遍、引号里的 `^` 会加倍，和 `-c` 值同一条限制。
        if cfg!(target_os = "windows") && args.iter().any(|arg| arg.contains(['"', '%', '^'])) {
            return Err("附加参数里有 Windows 终端传不过去的字符（\" % ^）".to_string());
        }
        for (arg, var) in args.into_iter().zip(USER_ARG_BATCH_VARS) {
            // 程序按 Windows 的规则拆命令行：收尾引号前的反斜杠要成对，否则 `C:\work\` 会把
            // 引号当成内容吞掉。
            let trailing = arg.len() - arg.trim_end_matches('\\').len();
            let quoted_value = format!("{arg}{}", "\\".repeat(trailing));
            self.batch_command.push_str(&format!(" \"%{var}%\""));
            self.batch_vars.push((var, quoted_value));
            self.args.push(arg);
        }
        Ok(())
    }
}

/// 附加参数在 .bat 里经这些变量传，也是附加参数的个数上限（各平台一样）。
const USER_ARG_BATCH_VARS: [&str; 16] = [
    "CC_SWITCH_ARG0",
    "CC_SWITCH_ARG1",
    "CC_SWITCH_ARG2",
    "CC_SWITCH_ARG3",
    "CC_SWITCH_ARG4",
    "CC_SWITCH_ARG5",
    "CC_SWITCH_ARG6",
    "CC_SWITCH_ARG7",
    "CC_SWITCH_ARG8",
    "CC_SWITCH_ARG9",
    "CC_SWITCH_ARG10",
    "CC_SWITCH_ARG11",
    "CC_SWITCH_ARG12",
    "CC_SWITCH_ARG13",
    "CC_SWITCH_ARG14",
    "CC_SWITCH_ARG15",
];

/// 把附加参数按空白拆开：单引号或双引号括起来的部分算同一个参数（引号本身去掉），没有
/// 反斜杠转义（Windows 路径里全是反斜杠）。只拆成参数，不经 shell，`;`、`&`、`|` 原样交给 CLI。
fn split_launch_args(input: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in input.chars() {
        if c.is_control() && !(quote.is_none() && c.is_whitespace()) {
            return Err("附加参数里不能有换行等控制字符".to_string());
        }
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            None => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("附加参数里的引号没有配对".to_string());
    }
    if in_word {
        args.push(current);
    }
    if args.len() > USER_ARG_BATCH_VARS.len() {
        return Err(format!("附加参数最多 {} 个", USER_ARG_BATCH_VARS.len()));
    }
    Ok(args)
}

/// Codex 的 `-c` 值在 .bat 里经这些变量传（值里可能有 `&` 之类 cmd 的特殊字符）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const CODEX_OVERRIDE_BATCH_VARS: [&str; 24] = [
    "CC_SWITCH_CODEX_C0",
    "CC_SWITCH_CODEX_C1",
    "CC_SWITCH_CODEX_C2",
    "CC_SWITCH_CODEX_C3",
    "CC_SWITCH_CODEX_C4",
    "CC_SWITCH_CODEX_C5",
    "CC_SWITCH_CODEX_C6",
    "CC_SWITCH_CODEX_C7",
    "CC_SWITCH_CODEX_C8",
    "CC_SWITCH_CODEX_C9",
    "CC_SWITCH_CODEX_C10",
    "CC_SWITCH_CODEX_C11",
    "CC_SWITCH_CODEX_C12",
    "CC_SWITCH_CODEX_C13",
    "CC_SWITCH_CODEX_C14",
    "CC_SWITCH_CODEX_C15",
    "CC_SWITCH_CODEX_C16",
    "CC_SWITCH_CODEX_C17",
    "CC_SWITCH_CODEX_C18",
    "CC_SWITCH_CODEX_C19",
    "CC_SWITCH_CODEX_C20",
    "CC_SWITCH_CODEX_C21",
    "CC_SWITCH_CODEX_C22",
    "CC_SWITCH_CODEX_C23",
];


/// Claude Code：把卡上的 env 和 `/model` 菜单写进临时 settings 文件，
/// `claude --settings <文件>` 启动。`drops` 是切到这张卡时会从 live 清掉的键，在临时文件里
/// 挡住（见 [`write_claude_config`]）。
fn claude_terminal_launch(
    env_vars: &[(String, String)],
    provider_id: &str,
    provider_name: &str,
) -> Result<ProviderLaunch, String> {
    let config_file = unique_temp_path(&format!("claude_{provider_id}"), "json");
    write_claude_config(&config_file, env_vars, provider_name)?;
    let config_name = config_file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(ProviderLaunch {
        title: "Using provider-specific claude config:",
        detail: Some(config_file.clone()),
        program: "claude",
        args: vec![
            "--settings".to_string(),
            config_file.to_string_lossy().into_owned(),
        ],
        // 配置文件和 .bat 同在 temp_dir（%TEMP% 含用户名），用 %~dp0 引用：目录由 cmd
        // 运行时展开，不进 .bat 字节，非 ASCII 用户名下也不会乱码（#8034）。
        batch_command: format!(
            "claude --settings \"%~dp0{}\"",
            escape_windows_batch_value(&config_name)
        ),
        env: Vec::new(),
        batch_vars: Vec::new(),
        cleanup: vec![config_file],
    })
}




/// macOS / Linux 的启动脚本里，环境变量先以这个前缀的名字读进来，再由 `env` 改名交给
/// 程序：用户 shell 的 rc 文件在中间执行，直接用原名会被 rc 里的 export 覆盖。
#[cfg_attr(windows, allow(dead_code))]
const LAUNCH_ENV_PREFIX: &str = "CC_SWITCH_LAUNCH_ENV_";

/// 同一进程里连开几个终端也不会撞名。
fn unique_temp_path(stem: &str, ext: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "{stem}_{}_{nanos:x}{}.{ext}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// 根据平台打开终端运行 `launch`。
fn launch_provider_terminal(launch: &ProviderLaunch, cwd: Option<&Path>) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (script_file, env_file) = write_unix_launcher(launch, cwd)?;
        #[cfg(target_os = "macos")]
        let result = launch_macos_terminal(&script_file);
        #[cfg(target_os = "linux")]
        let result = launch_linux_terminal(&script_file);
        if result.is_err() {
            for file in launch.cleanup.iter().chain([&script_file, &env_file]) {
                let _ = std::fs::remove_file(file);
            }
        }
        result
    }

    #[cfg(target_os = "windows")]
    {
        let result = launch_windows_terminal(launch, cwd);
        if result.is_err() {
            for file in &launch.cleanup {
                let _ = std::fs::remove_file(file);
            }
        }
        result
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (launch, cwd);
        Err("不支持的操作系统".to_string())
    }
}

/// shell 里的一个词：只含安全字符时原样写，否则加单引号。
#[cfg_attr(windows, allow(dead_code))]
fn shell_word(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '='));
    if plain {
        value.to_string()
    } else {
        shell_single_quote(value)
    }
}

/// 同上，参数按用户 shell 的规则加引号：这条命令由用户 shell 解析（见
/// `build_provider_command_line`），附加参数又是用户随手填的文本。
#[cfg_attr(windows, allow(dead_code))]
fn unix_launch_command_in(launch: &ProviderLaunch, shell: &str) -> String {
    let mut words = Vec::new();
    if !launch.env.is_empty() {
        words.push("env".to_string());
        for (index, (name, value)) in launch.env.iter().enumerate() {
            // 中转用的变量也不留给程序。
            words.push(format!("-u {LAUNCH_ENV_PREFIX}{index}"));
            if value.is_none() {
                words.push(format!("-u {name}"));
            }
        }
        for (index, (name, value)) in launch.env.iter().enumerate() {
            if value.is_some() {
                words.push(format!("{name}=\"${LAUNCH_ENV_PREFIX}{index}\""));
            }
        }
    }
    words.push(launch.program.to_string());
    words.extend(launch.args.iter().map(|arg| user_shell_word(shell, arg)));
    words.join(" ")
}

/// 0600 环境变量文件的内容，启动脚本 `.` 进来后立即删除。
#[cfg_attr(windows, allow(dead_code))]
fn unix_launch_env_file(launch: &ProviderLaunch) -> String {
    launch
        .env
        .iter()
        .enumerate()
        .filter_map(|(index, (_, value))| {
            value
                .as_ref()
                .map(|value| format!("{LAUNCH_ENV_PREFIX}{index}={}\n", shell_single_quote(value)))
        })
        .collect()
}

/// 启动脚本：读进环境变量文件并删掉它，跑启动命令，删掉临时文件，最后换成用户的交互 shell。
#[cfg_attr(windows, allow(dead_code))]
fn build_unix_launcher_script(
    launch: &ProviderLaunch,
    shell: &str,
    cwd: Option<&Path>,
    script_file: &Path,
    env_file: Option<&Path>,
) -> String {
    let mut temp_files: Vec<String> = launch
        .cleanup
        .iter()
        .map(PathBuf::as_path)
        .chain(env_file)
        .map(|file| shell_single_quote(&file.to_string_lossy()))
        .collect();
    temp_files.push(shell_single_quote(&script_file.to_string_lossy()));
    let remove_temp_files = format!("rm -f -- {}", temp_files.join(" "));

    let mut script = String::from("#!/usr/bin/env sh\n");
    script.push_str(&format!(
        "trap {} EXIT\n",
        shell_single_quote(&remove_temp_files)
    ));
    if let Some(env_file) = env_file {
        let quoted = shell_single_quote(&env_file.to_string_lossy());
        script.push_str(&format!("set -a\n. {quoted}\nset +a\nrm -f -- {quoted}\n"));
    }
    script.push_str(&format!("echo {}\n", shell_single_quote(launch.title)));
    if let Some(detail) = &launch.detail {
        script.push_str(&format!(
            "echo {}\n",
            shell_single_quote(&detail.to_string_lossy())
        ));
    }
    script.push_str(&build_provider_command_line(
        shell,
        &unix_launch_command_in(launch, shell),
        cwd,
    ));
    script.push('\n');
    // exec 不会触发 EXIT trap，所以在换成交互 shell 之前自己删。
    script.push_str(&remove_temp_files);
    script.push('\n');
    let launch_vars: Vec<String> = (0..launch.env.len())
        .map(|index| format!("{LAUNCH_ENV_PREFIX}{index}"))
        .collect();
    if !launch_vars.is_empty() {
        script.push_str(&format!("unset {}\n", launch_vars.join(" ")));
    }
    script.push_str(&build_final_shell_cd_command(shell, cwd));
    script.push_str(&build_exec_line(shell, cwd));
    script.push('\n');
    script
}

/// 写好启动脚本（和需要时的 0600 环境变量文件），返回两者路径。
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn write_unix_launcher(
    launch: &ProviderLaunch,
    cwd: Option<&Path>,
) -> Result<(PathBuf, PathBuf), String> {
    use std::os::unix::fs::PermissionsExt;

    let shell = get_user_shell();
    let script_file = unique_temp_path("cc_switch_launcher", "sh");
    let env_file = unique_temp_path("cc_switch_launch_env", "sh");
    let env_content = unix_launch_env_file(launch);
    let has_env = !env_content.is_empty();
    if has_env {
        crate::config::atomic_write_private(&env_file, env_content.as_bytes())
            .map_err(|e| format!("写入启动环境失败: {e}"))?;
    }
    let script_content = build_unix_launcher_script(
        launch,
        &shell,
        cwd,
        &script_file,
        has_env.then_some(env_file.as_path()),
    );
    std::fs::write(&script_file, &script_content).map_err(|e| format!("写入启动脚本失败: {e}"))?;
    std::fs::set_permissions(&script_file, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("设置脚本权限失败: {e}"))?;
    Ok((script_file, env_file))
}

/// 写入 claude 配置文件（含凭据，0600）
///
/// 这个文件叠在用户级 `settings.json` 上：`env` 按键合并、顶层按键覆盖。所以：
/// - `modelPicker` 一定要写：它按层级整个胜出，不写的话路由 / 聚合模式的菜单会漏进这个
///   终端，选了就把别家的模型发给这张卡；
/// - 切换会清掉、卡上又没有的键（`drops`）写成空值挡住：Claude Code 把空串当没设（2.1.314
///   实测：代理的占位凭据不再随请求发出，没配的档位回到内置模型）。
/// 把这张卡的配置写成 `claude --settings <文件>` 用的临时文件。
///
/// `--settings` 是叠在用户级 `settings.json` 之上的：用户级里那些指向别家的键（地址、
/// 凭据、模型名）不会因为这里没写而被顶掉，所以要显式挡住 —— 关键字段逐项写空，`env`
/// 里的空字符串等于没有，顶层写 `null` 等于未设置。`modelPicker` 按层级整个胜出，写一个
/// 空菜单让 `/model` 显示 Claude Code 内置的那份。
fn write_claude_config(
    config_file: &std::path::Path,
    env_vars: &[(String, String)],
    _provider_name: &str,
) -> Result<(), String> {
    use crate::floor;
    use serde_json::Value;

    let mut config_obj = serde_json::Map::new();
    let mut env_obj = serde_json::Map::new();

    for (key, value) in env_vars {
        env_obj.insert(key.clone(), Value::String(value.clone()));
    }

    for key in floor::CLAUDE_FLOOR_ENV_KEYS
        .iter()
        .chain(floor::CLAUDE_PROTOCOL_SELECTORS)
        .chain(floor::CLAUDE_EXCLUSIVE_ENV)
    {
        env_obj
            .entry((*key).to_string())
            .or_insert_with(|| Value::String(String::new()));
    }
    for key in floor::CLAUDE_FLOOR_TOP {
        config_obj.insert((*key).to_string(), Value::Null);
    }
    config_obj.insert("env".to_string(), Value::Object(env_obj));
    config_obj.insert(
        "modelPicker".to_string(),
        serde_json::json!({"replaceBuiltInOptions": false, "options": []}),
    );

    let config_json =
        serde_json::to_string_pretty(&config_obj).map_err(|e| format!("序列化配置失败: {e}"))?;

    crate::config::atomic_write_private(config_file, config_json.as_bytes())
        .map_err(|e| format!("写入配置文件失败: {e}"))
}

/// macOS: 根据用户首选终端启动
#[cfg(target_os = "macos")]
fn launch_macos_terminal(script_file: &std::path::Path) -> Result<(), String> {
    let preferred = crate::settings::get_preferred_terminal();
    let terminal = preferred.as_deref().unwrap_or("terminal");
    let script_file = script_file.to_path_buf();

    // Try the preferred terminal first, fall back to Terminal.app if it fails
    // Note: Kitty doesn't need the -e flag, others do
    let result = match terminal {
        "iterm2" => launch_macos_iterm2(&script_file),
        "warp" => launch_macos_warp(&script_file),
        "alacritty" => launch_macos_open_app("Alacritty", &script_file, true),
        "kitty" => launch_macos_open_app("kitty", &script_file, false),
        "ghostty" => launch_macos_ghostty(&script_file),
        "otty" => launch_macos_otty(&script_file),
        "wezterm" => launch_macos_open_app("WezTerm", &script_file, true),
        "kaku" => launch_macos_open_app("Kaku", &script_file, true),
        _ => launch_macos_terminal_app(&script_file),
    };

    // If preferred terminal fails and it's not the default, try Terminal.app as fallback
    if result.is_err() && terminal != "terminal" {
        log::warn!(
            "首选终端 {} 启动失败，回退到 Terminal.app: {:?}",
            terminal,
            result.as_ref().err()
        );
        return launch_macos_terminal_app(&script_file);
    }

    result
}

/// Escape a value as an AppleScript string literal.
#[cfg(target_os = "macos")]
fn applescript_string_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Build the launcher command literal used by AppleScript.
#[cfg(target_os = "macos")]
fn applescript_launcher_command(script_file: &std::path::Path) -> String {
    applescript_string_literal(&format!(
        "sh {}",
        shell_single_quote(&script_file.to_string_lossy())
    ))
}

/// Build a launcher command that replaces the terminal-created shell session.
#[cfg(target_os = "macos")]
fn applescript_exec_launcher_command(script_file: &std::path::Path) -> String {
    applescript_string_literal(&format!(
        "exec sh {}",
        shell_single_quote(&script_file.to_string_lossy())
    ))
}

/// macOS: Terminal.app AppleScript.
/// A cold `activate` creates a default empty window before `do script` opens the command session.
/// Use `launch` for cold starts so `do script` can create the only new session without reusing restored windows.
#[cfg(target_os = "macos")]
fn build_macos_terminal_applescript(script_file: &std::path::Path) -> String {
    format!(
        r#"set launcher_script to {launcher}
set was_running to application "Terminal" is running
tell application "Terminal"
    if was_running then
        activate
        do script launcher_script
    else
        launch
        do script launcher_script
        activate
    end if
end tell"#,
        launcher = applescript_exec_launcher_command(script_file)
    )
}

/// Run AppleScript through `osascript -e` with shared error handling.
#[cfg(target_os = "macos")]
fn run_terminal_osascript(applescript: &str, terminal_label: &str) -> Result<(), String> {
    use std::process::Command;

    let output = Command::new("osascript")
        .arg("-e")
        .arg(applescript)
        .output()
        .map_err(|e| format!("执行 osascript 失败: {e}"))?;

    if !output.status.success() {
        let stderr = decode_command_output(&output.stderr);
        return Err(format!(
            "{terminal_label} 执行失败 (exit code: {:?}): {}",
            output.status.code(),
            stderr
        ));
    }

    Ok(())
}

/// macOS: Terminal.app
#[cfg(target_os = "macos")]
fn launch_macos_terminal_app(script_file: &std::path::Path) -> Result<(), String> {
    run_terminal_osascript(
        &build_macos_terminal_applescript(script_file),
        "Terminal.app",
    )
}

#[cfg(target_os = "macos")]
fn launch_macos_otty(script_file: &std::path::Path) -> Result<(), String> {
    use std::process::Command;

    let otty_cli = find_macos_otty_cli().ok_or_else(|| {
        "未找到 Otty CLI。请将 Otty 安装到 /Applications 或 ~/Applications。".to_string()
    })?;

    let command = build_macos_dash_c_command(script_file);
    let tab_result = Command::new(&otty_cli)
        .args(["tab", "new", "--window", "0", "--command", &command])
        .output()
        .map_err(|e| format!("启动 Otty CLI 失败: {e}"))?;

    if tab_result.status.success() {
        return Ok(());
    }

    log::debug!(
        "Otty 新建 Tab 失败，改为新建窗口: {}",
        decode_command_output(&tab_result.stderr)
    );

    let window_result = Command::new(&otty_cli)
        .args(["open", "--command", &command])
        .output()
        .map_err(|e| format!("启动 Otty CLI 失败: {e}"))?;

    if window_result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Otty 新建窗口失败 (exit code: {:?}): {}",
            window_result.status.code(),
            decode_command_output(&window_result.stderr)
        ))
    }
}

#[cfg(target_os = "macos")]
fn find_macos_otty_cli() -> Option<std::path::PathBuf> {
    macos_otty_cli_candidates()
        .into_iter()
        .find(|path| path.is_file() && is_executable_file(path))
}

#[cfg(target_os = "macos")]
fn macos_otty_cli_candidates() -> Vec<std::path::PathBuf> {
    let mut candidates = vec![std::path::PathBuf::from(
        "/Applications/Otty.app/Contents/MacOS/otty-cli",
    )];

    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            std::path::PathBuf::from(home).join("Applications/Otty.app/Contents/MacOS/otty-cli"),
        );
    }

    candidates.push(std::path::PathBuf::from("/usr/local/bin/otty"));
    candidates.push(std::path::PathBuf::from("/opt/homebrew/bin/otty"));

    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            candidates.push(directory.join("otty"));
            candidates.push(directory.join("otty-cli"));
        }
    }

    candidates
}

/// macOS: iTerm2
#[cfg(target_os = "macos")]
fn build_macos_iterm2_applescript(script_file: &std::path::Path) -> String {
    format!(
        r#"set launcher_script to {launcher}
set was_running to application "iTerm" is running
tell application "iTerm"
    if was_running then
        activate
        if (count of windows) = 0 then
            create window with default profile
        else
            tell current window
                create tab with default profile
            end tell
        end if
    else
        activate
        set waited to 0
        repeat while (count of windows) = 0
            delay 0.1
            set waited to waited + 1
            if waited >= 30 then exit repeat
        end repeat
        if (count of windows) = 0 then
            create window with default profile
        end if
    end if
    tell current session of current window
        write text launcher_script
    end tell
end tell"#,
        launcher = applescript_exec_launcher_command(script_file)
    )
}

/// macOS: iTerm2
#[cfg(target_os = "macos")]
fn launch_macos_iterm2(script_file: &std::path::Path) -> Result<(), String> {
    run_terminal_osascript(&build_macos_iterm2_applescript(script_file), "iTerm2")
}

/// Keep the launcher path inside a `sh -c` string.
/// A bare `.sh` passed through `open --args` may also be opened as a document.
#[cfg(target_os = "macos")]
fn build_macos_dash_c_command(script_file: &std::path::Path) -> String {
    format!(
        "exec sh {}",
        shell_single_quote(&script_file.to_string_lossy())
    )
}

/// macOS: Ghostty.
/// Warm starts use AppleScript to create one command window.
/// Cold starts use `initial-command` so the first default surface runs the launcher.
/// Do not use `initial-window=false` plus `new window`: cold launch can still create the default window first.
#[cfg(target_os = "macos")]
fn build_macos_ghostty_applescript(script_file: &std::path::Path) -> String {
    format!(
        r#"set launcher_command to {launcher}
set was_running to application "Ghostty" is running
if was_running then
    tell application "Ghostty"
        new window with configuration {{command:launcher_command}}
    end tell
else
    do shell script "open -na Ghostty --args --quit-after-last-window-closed=true " & quoted form of ("--initial-command=" & launcher_command)
end if
"#,
        launcher = applescript_launcher_command(script_file)
    )
}

/// macOS: Ghostty
#[cfg(target_os = "macos")]
fn launch_macos_ghostty(script_file: &std::path::Path) -> Result<(), String> {
    match run_terminal_osascript(&build_macos_ghostty_applescript(script_file), "Ghostty") {
        Ok(()) => Ok(()),
        Err(applescript_error) => {
            log::warn!(
                "Ghostty AppleScript launch failed, falling back to open -na: {applescript_error}"
            );
            launch_macos_open_app("Ghostty", script_file, true)
        }
    }
}

/// macOS: 使用 open -na 启动支持 --args 参数的终端（Alacritty/Kitty/WezTerm/Kaku）
#[cfg(target_os = "macos")]
fn launch_macos_open_app(
    app_name: &str,
    script_file: &std::path::Path,
    use_e_flag: bool,
) -> Result<(), String> {
    use std::process::Command;

    let mut cmd = Command::new("open");
    cmd.arg("-na").arg(app_name).arg("--args");

    if use_e_flag {
        cmd.arg("-e");
    }
    // Keep the script path inside `sh -c`; a trailing bare `.sh` can be opened as a document.
    cmd.arg("sh")
        .arg("-c")
        .arg(build_macos_dash_c_command(script_file));

    let output = cmd
        .output()
        .map_err(|e| format!("启动 {app_name} 失败: {e}"))?;

    if !output.status.success() {
        let stderr = decode_command_output(&output.stderr);
        return Err(format!(
            "{} 启动失败 (exit code: {:?}): {}",
            app_name,
            output.status.code(),
            stderr
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_macos_warp(script_file: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let mut cmd = Command::new("open");
    cmd.arg("-a").arg("Warp");

    // Warp URI scheme cannot work well with script_file, because:
    //
    // 1. script_file's name ends up with .sh, so Warp would open the file rather than execute it
    // 2. script_file has no execution permission, so we need to add one more indirection
    let mut second_script_file = tempfile::Builder::new()
        .disable_cleanup(true)
        .permissions(std::fs::Permissions::from_mode(0o755))
        .tempfile()
        .map_err(|e| format!("Failed to create temporary script file: {e}"))?;

    writeln!(
        &mut second_script_file,
        r#"#!/usr/bin/env sh

        rm -- "$0"

        exec sh {quoted_script}
        "#,
        quoted_script = shell_single_quote(&script_file.to_string_lossy()),
    )
    .map_err(|e| format!("Failed to write to temporary script file for Warp: {e}"))?;

    let mut warp_url = url::Url::parse("warp://action/new_tab").unwrap();
    warp_url
        .query_pairs_mut()
        .append_pair("path", &second_script_file.path().to_string_lossy());
    let warp_url = warp_url.to_string();
    cmd.arg(warp_url);

    let output = cmd.output().map_err(|e| format!("启动 Warp 失败: {e}"))?;
    if !output.status.success() {
        let stderr = decode_command_output(&output.stderr);
        return Err(format!(
            "Warp 启动失败 (exit code: {:?}): {}",
            output.status.code(),
            stderr
        ));
    }

    Ok(())
}

/// Linux: 根据用户首选终端启动
#[cfg(target_os = "linux")]
fn launch_linux_terminal(script_file: &std::path::Path) -> Result<(), String> {
    use std::process::Command;

    let preferred = crate::settings::get_preferred_terminal();

    // Default terminal list with their arguments
    let default_terminals = [
        ("gnome-terminal", vec!["--"]),
        ("konsole", vec!["-e"]),
        ("xfce4-terminal", vec!["-e"]),
        ("mate-terminal", vec!["--"]),
        ("lxterminal", vec!["-e"]),
        ("alacritty", vec!["-e"]),
        ("kitty", vec!["-e"]),
        ("ghostty", vec!["-e"]),
    ];

    // Build terminal list: preferred terminal first (if specified), then defaults
    let terminals_to_try: Vec<(&str, Vec<&str>)> = if let Some(ref pref) = preferred {
        // Find the preferred terminal's args from default list
        let pref_args = default_terminals
            .iter()
            .find(|(name, _)| *name == pref.as_str())
            .map(|(_, args)| args.to_vec())
            .unwrap_or_else(|| vec!["-e"]); // Default args for unknown terminals

        let mut list = vec![(pref.as_str(), pref_args)];
        // Add remaining terminals as fallbacks
        for (name, args) in &default_terminals {
            if *name != pref.as_str() {
                list.push((*name, args.to_vec()));
            }
        }
        list
    } else {
        default_terminals
            .iter()
            .map(|(name, args)| (*name, args.to_vec()))
            .collect()
    };

    let mut last_error = String::from("未找到可用的终端");

    for (terminal, args) in terminals_to_try {
        // Check if terminal exists in common paths
        let terminal_exists = std::path::Path::new(&format!("/usr/bin/{}", terminal)).exists()
            || std::path::Path::new(&format!("/bin/{}", terminal)).exists()
            || std::path::Path::new(&format!("/usr/local/bin/{}", terminal)).exists()
            || which_command(terminal);

        if terminal_exists {
            let result = Command::new(terminal)
                .args(&args)
                .arg("sh")
                .arg(script_file.to_string_lossy().as_ref())
                .spawn();

            match result {
                Ok(_) => return Ok(()),
                Err(e) => {
                    last_error = format!("执行 {} 失败: {}", terminal, e);
                }
            }
        }
    }

    Err(last_error)
}

/// Check if a command exists using `which`
#[cfg(target_os = "linux")]
fn which_command(cmd: &str) -> bool {
    use std::process::Command;
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Windows: 根据用户首选终端启动
#[cfg(target_os = "windows")]
fn launch_windows_terminal(launch: &ProviderLaunch, cwd: Option<&Path>) -> Result<(), String> {
    let preferred = crate::settings::get_preferred_terminal();
    let terminal = preferred.as_deref().unwrap_or("cmd");

    let bat_file = unique_temp_path("cc_switch_launcher", "bat");
    let (cwd_command, cwd_env) = build_windows_cwd_command(cwd);
    let mut launch_env: Vec<(String, String)> = cwd_env
        .map(|value| (WINDOWS_CWD_ENV.to_string(), value))
        .into_iter()
        .collect();
    let (set_lines, extra_env) = build_windows_launch_env(launch);
    launch_env.extend(extra_env);
    let launch_env: Vec<(&str, &str)> = launch_env
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let cleanup: String = launch
        .cleanup
        .iter()
        .filter_map(|file| file.file_name())
        .map(|name| {
            format!(
                "del \"%~dp0{}\" >nul 2>&1\r\n",
                escape_windows_batch_value(&name.to_string_lossy())
            )
        })
        .collect();

    // setlocal 段里设的变量（含 Key）在程序退出后随 endlocal 撤掉，不留在这个 cmd 会话里。
    // 注册表里开了延迟展开的机器上，值里的 `!名字!` 会被替换：这一段显式关掉。
    let content = format!(
        "@echo off\r\n{cwd_command}setlocal DisableDelayedExpansion\r\n{set_lines}echo {title}\r\n{command}\r\nendlocal\r\n{cleanup}del \"%~f0\" >nul 2>&1\r\n",
        title = escape_windows_batch_value(launch.title),
        command = launch.batch_command,
    );

    std::fs::write(&bat_file, &content).map_err(|e| format!("写入批处理文件失败: {e}"))?;

    let bat_path = bat_file.to_string_lossy();
    let ps_cmd = format!("& '{}'", bat_path);

    // Try the preferred terminal first
    let result = match terminal {
        "powershell" => run_windows_start_command_with_env(
            &[
                windows_powershell_program(&effective_path_string()),
                "-NoExit",
                "-Command",
                &ps_cmd,
            ],
            "PowerShell",
            &launch_env,
        ),
        "wt" => run_windows_start_command_with_env(
            &["wt", "cmd", "/K", &bat_path],
            "Windows Terminal",
            &launch_env,
        ),
        // "cmd" or default
        _ => run_windows_start_command_with_env(&["cmd", "/K", &bat_path], "cmd", &launch_env),
    };

    // If preferred terminal fails and it's not the default, try cmd as fallback
    if result.is_err() && terminal != "cmd" {
        log::warn!(
            "首选终端 {} 启动失败，回退到 cmd: {:?}",
            terminal,
            result.as_ref().err()
        );
        return run_windows_start_command_with_env(&["cmd", "/K", &bat_path], "cmd", &launch_env);
    }

    result
}

#[cfg_attr(windows, allow(dead_code))]
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// fish 的单引号里 `\'` 和 `\\` 仍是转义，POSIX 的 `'"'"'` 写法会让引号错位、接出别的命令。
#[cfg_attr(windows, allow(dead_code))]
fn is_fish_shell(shell: &str) -> bool {
    shell.rsplit('/').next().unwrap_or(shell) == "fish"
}

/// 交给用户 shell（而不是启动脚本的 sh）解析的一段文本加引号。
#[cfg_attr(windows, allow(dead_code))]
fn user_shell_quote(shell: &str, value: &str) -> String {
    if is_fish_shell(shell) {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        shell_single_quote(value)
    }
}

/// `shell_word` 的用户 shell 版本：只含安全字符时原样写。
#[cfg_attr(windows, allow(dead_code))]
fn user_shell_word(shell: &str, value: &str) -> String {
    let word = shell_word(value);
    // zsh 把开头的 `=名字` 展开成命令路径（`=sh` → `/bin/sh`），找不到还会直接报错。
    if word == value && !value.starts_with('=') {
        word
    } else {
        user_shell_quote(shell, value)
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn is_windows_unc_path(path: &str) -> bool {
    path.starts_with(r"\\")
}

/// .bat 里 `setlocal` 段的 `set` 行，以及要经启动进程环境传入的变量。纯 ASCII 的值直接
/// 写进文件（`%` 双写）；含非 ASCII 的按 #8034 的办法经环境变量转一手，免得按代码页乱码。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn build_windows_launch_env(launch: &ProviderLaunch) -> (String, Vec<(String, String)>) {
    let mut lines = String::new();
    let mut carried = Vec::new();
    let entries = launch
        .env
        .iter()
        .map(|(name, value)| (*name, value.clone()))
        .chain(
            launch
                .batch_vars
                .iter()
                .map(|(name, value)| (*name, Some(value.clone()))),
        );
    for (index, (name, value)) in entries.enumerate() {
        match value {
            None => lines.push_str(&format!("set \"{name}=\"\r\n")),
            Some(value) if value.is_ascii() => {
                lines.push_str(&format!("set \"{name}={}\"\r\n", value.replace('%', "%%")))
            }
            Some(value) => {
                let carrier = format!("CC_SWITCH_INTERNAL_ENV_{index}");
                lines.push_str(&format!(
                    "set \"{name}=%{carrier}%\"\r\nset \"{carrier}=\"\r\n"
                ));
                carried.push((carrier, value));
            }
        }
    }
    (lines, carried)
}

/// 非 ASCII 的项目目录经这个环境变量传给 Claude 启动 .bat。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_CWD_ENV: &str = "CC_SWITCH_INTERNAL_CWD";

/// 生成切到项目目录的 .bat 行，第二项是需要给启动进程设置的 `WINDOWS_CWD_ENV` 值。
///
/// cmd 按控制台代码页解码 .bat 字节，非 ASCII 路径以 UTF-8 写进文件会乱码（#8034）；
/// 环境变量在 Windows 内部保持 Unicode，所以这类路径改由环境变量传入。纯 ASCII
/// 路径照旧写进文件，不依赖终端继承环境变量（旧版 Windows Terminal 不继承）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn build_windows_cwd_command_str(path: &str) -> (String, Option<String>) {
    let (target, env_value, cleanup) = if path.is_ascii() {
        (escape_windows_batch_value(path), None, String::new())
    } else {
        (
            format!("%{WINDOWS_CWD_ENV}%"),
            Some(path.to_string()),
            format!("set \"{WINDOWS_CWD_ENV}=\"\r\n"),
        )
    };

    let command = if is_windows_unc_path(path) {
        // `cmd.exe` cannot make a UNC path current via `cd`; `pushd` maps it first.
        format!("pushd \"{target}\" || exit /b 1\r\n{cleanup}")
    } else {
        format!("cd /d \"{target}\" || exit /b 1\r\n{cleanup}")
    };
    (command, env_value)
}

#[cfg(target_os = "windows")]
fn build_windows_cwd_command(cwd: Option<&Path>) -> (String, Option<String>) {
    cwd.map(|dir| build_windows_cwd_command_str(&dir.to_string_lossy()))
        .unwrap_or_default()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn escape_windows_batch_value(value: &str) -> String {
    value
        .replace('^', "^^")
        .replace('%', "%%")
        .replace('&', "^&")
        .replace('|', "^|")
        .replace('<', "^<")
        .replace('>', "^>")
        .replace('(', "^(")
        .replace(')', "^)")
}
/// Windows: Run a start command with common error handling
#[cfg(target_os = "windows")]
fn run_windows_start_command(args: &[&str], terminal_name: &str) -> Result<(), String> {
    run_windows_start_command_with_env(args, terminal_name, &[])
}

/// 组装 `cmd /C start ...`。终端里跑的 `claude` 等命令按这里的 PATH 查找：CC Switch
/// 被 MSI 自动重启后只剩机器级 PATH（#6061），或是在装 CLI 之前就已启动，进程 PATH
/// 都会缺用户级条目（#3877、#8446）。和检测、生命周期命令（#8173）一样用补全后的 PATH，
/// 只影响启动的终端，不改 CC Switch 自身的环境；Windows Terminal 从命令行开新标签时
/// 也沿用调用方的环境。
#[cfg(target_os = "windows")]
fn windows_start_command(
    args: &[&str],
    envs: &[(&str, &str)],
    effective_path: &str,
) -> std::process::Command {
    let mut command = std::process::Command::new("cmd");
    command
        .arg("/C")
        .arg("start")
        .args(args)
        .env("PATH", effective_path)
        .envs(envs.iter().copied())
        .creation_flags(CREATE_NO_WINDOW);
    command
}

/// 同 `run_windows_start_command`，额外给启动的终端设置环境变量（`start` 会继承）。
///
/// stdio 指向 NUL、只等 `cmd` 的退出码，不能用 `output()` 捕获：`start` 打开的终端会
/// 继承这里的 stdout/stderr 管道句柄，管道要等那个窗口和里面的进程全部退出才到 EOF，
/// 调用就一直挂到用户关掉终端，界面也一直转圈（#6712）。启动失败仍能从退出码看出来。
#[cfg(target_os = "windows")]
fn run_windows_start_command_with_env(
    args: &[&str],
    terminal_name: &str,
    envs: &[(&str, &str)],
) -> Result<(), String> {
    use std::process::Stdio;

    let status = windows_start_command(args, envs, &effective_path_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("启动 {} 失败: {e}", terminal_name))?;

    if !status.success() {
        return Err(format!(
            "{} 启动失败 (exit code: {:?})",
            terminal_name,
            status.code()
        ));
    }

    Ok(())
}

/// 首选终端为 PowerShell 时启动哪个程序。`powershell` 永远是系统自带的 Windows
/// PowerShell 5.1；装了 PowerShell 7+ 的用户默认用的是 `pwsh`，所以补全后的 PATH 里
/// 有 `pwsh.exe` 就用它，没有才退回 `powershell`。Microsoft Store 版的 `pwsh.exe` 是
/// App Execution Alias（跟随不了的重解析点），`Path::exists` 会判成不存在，这里用
/// `symlink_metadata`。
#[cfg(target_os = "windows")]
fn windows_powershell_program(path: &str) -> &'static str {
    let has_pwsh = std::env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .any(|dir| std::fs::symlink_metadata(dir.join("pwsh.exe")).is_ok());
    if has_pwsh {
        "pwsh"
    } else {
        "powershell"
    }
}


/// 设置窗口主题（Windows/macOS 标题栏颜色）
/// theme: "dark" | "light" | "system"
#[tauri::command]
pub async fn set_window_theme(window: tauri::Window, theme: String) -> Result<(), String> {
    use tauri::Theme;

    let tauri_theme = match theme.as_str() {
        "dark" => Some(Theme::Dark),
        "light" => Some(Theme::Light),
        _ => None, // system default
    };

    window.set_theme(tauri_theme).map_err(|e| e.to_string())
}

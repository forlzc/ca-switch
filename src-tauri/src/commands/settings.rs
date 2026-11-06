#![allow(non_snake_case)]

use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// 应用更新下载进度（通过 `update-download-progress` 事件发给前端）。
#[derive(Clone, serde::Serialize)]
struct UpdateDownloadProgress {
    downloaded: u64,
    total: Option<u64>,
}

fn merge_settings_for_save(
    mut incoming: crate::settings::AppSettings,
    existing: &crate::settings::AppSettings,
) -> crate::settings::AppSettings {
    match (&mut incoming.webdav_sync, &existing.webdav_sync) {
        // incoming 没有 webdav → 保留现有
        (None, _) => {
            incoming.webdav_sync = existing.webdav_sync.clone();
        }
        // incoming 有 webdav 但密码为空，且现有有密码 → 填回现有密码
        // （get_settings_for_frontend 总是清空密码，所以通过 save_settings
        //   传入的空密码意味着"保持现有"而非"用户主动清空"）
        (Some(incoming_sync), Some(existing_sync))
            if incoming_sync.password.is_empty() && !existing_sync.password.is_empty() =>
        {
            incoming_sync.password = existing_sync.password.clone();
        }
        _ => {}
    }
    match (&mut incoming.s3_sync, &existing.s3_sync) {
        // incoming 没有 s3 → 保留现有
        (None, _) => {
            incoming.s3_sync = existing.s3_sync.clone();
        }
        // incoming 有 s3 但密钥为空，且现有有密钥 → 填回现有密钥
        (Some(incoming_sync), Some(existing_sync))
            if incoming_sync.secret_access_key.is_empty()
                && !existing_sync.secret_access_key.is_empty() =>
        {
            incoming_sync.secret_access_key = existing_sync.secret_access_key.clone();
        }
        _ => {}
    }
    // 客户端地址只经 set_proxy_client_host 保存（它还要重写客户端）；全量保存带的是前端
    // 缓存里的旧值，透传会把刚保存的地址冲掉。
    incoming.proxy_client_host = existing.proxy_client_host.clone();
    // 轻量模式状态只由进入 / 退出轻量模式时写，前端缓存里的值总是旧的。
    incoming.lightweight_mode = existing.lightweight_mode;
    incoming
}

/// 获取设置
#[tauri::command]
pub async fn get_settings() -> Result<crate::settings::AppSettings, String> {
    Ok(crate::settings::get_settings_for_frontend())
}

/// 保存设置
#[tauri::command]
pub async fn save_settings(
    app: tauri::AppHandle,
    _state: tauri::State<'_, crate::store::AppState>,
    settings: crate::settings::AppSettings,
) -> Result<bool, String> {
    let existing = crate::settings::get_settings();
    let merged = merge_settings_for_save(settings, &existing);
    let tray_style_changed = merged.native_tray_menu != existing.native_tray_menu;
    crate::settings::update_settings(merged).map_err(|e| e.to_string())?;

    // 原生托盘菜单 / 托盘面板当场切换，不用重启。
    if tray_style_changed {
        crate::tray::apply_tray_style(&app);
    }
    Ok(true)
}

/// 重启应用程序（当 app_config_dir 变更后使用）
#[tauri::command]
pub async fn restart_app(app: AppHandle) -> Result<bool, String> {
    crate::save_window_state_before_exit(&app);

    // 在后台延迟重启，让函数有时间返回响应
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        // 本命令用于 app_config_dir 变更后的重启：新实例会切到新数据库，拿不到
        // 旧库里的 Live 备份，无法恢复被接管的 Live 配置。因此必须趁旧实例的
        // 事件循环仍存活，在这里同步完成恢复（保留代理状态，新实例启动时自动
        // 重新接管）。
        crate::cleanup_before_exit(&app).await;
        // 与自更新共用 restart_process，而不是 app.restart()：macOS 上经
        // LaunchServices 启动，新窗口才能回到前台。
        crate::restart_process(&app);
    });
    Ok(true)
}

/// 下载并安装应用更新，然后由后端直接重启应用。
///
/// macOS 更新会原地替换 `.app` bundle。如果先返回前端、再让旧 WebView 调
/// `process.relaunch()`，旧进程可能已经处在 bundle 被替换后的不稳定窗口期。
/// 这里把退出清理、安装和重启串在同一个后端流程中，避免依赖旧前端继续执行。
#[tauri::command]
pub async fn install_update_and_restart(app: AppHandle) -> Result<bool, String> {
    let builder = app.updater_builder();
    // Windows 上 updater 以 `msiexec /passive /promptrestart` 安装 MSI。passive 没有模态框、
    // 未指定重启选项时，安装目录里有文件被占用，Windows Installer 会不经确认直接重启系统
    // （#8534）。`/promptrestart` 只对应 REBOOTPROMPT，挡不住这一点，因此显式追加
    // REBOOT=ReallySuppress（即 `/norestart`）。旧版本的 updater 不带这个参数，
    // 由 MSI 模板自身的 REBOOT 属性兜底。
    #[cfg(target_os = "windows")]
    let builder = builder.installer_arg("REBOOT=ReallySuppress");
    let updater = builder
        .build()
        .map_err(|e| format!("初始化更新器失败: {e}"))?;

    let Some(update) = updater
        .check()
        .await
        .map_err(|e| format!("检查更新失败: {e}"))?
    else {
        return Ok(false);
    };

    log::info!("开始下载应用更新: {}", update.version);
    let progress_handle = app.clone();
    let mut downloaded: u64 = 0;
    let bytes = update
        .download(
            move |chunk_len, content_len| {
                downloaded = downloaded.saturating_add(chunk_len as u64);
                let _ = progress_handle.emit(
                    "update-download-progress",
                    UpdateDownloadProgress {
                        downloaded,
                        total: content_len,
                    },
                );
            },
            || {},
        )
        .await
        .map_err(|e| format!("下载更新失败: {e}"))?;

    log::info!("开始安装应用更新: {}", update.version);

    #[cfg(target_os = "windows")]
    {
        // Windows updater 会在 install() 内启动安装器并直接退出当前进程
        // （插件内部 std::process::exit(0)，绕过 TrayIcon::drop、不发
        // NIM_DELETE，会残留死图标——与托盘"退出"路径相同的问题）。
        // 因此清理只能放在 install 前执行，且必须显式移除托盘图标。
        crate::save_window_state_before_exit(&app);
        crate::cleanup_before_exit(&app).await;
        crate::remove_tray_icon_before_exit(&app);
        crate::destroy_single_instance_lock(&app);
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        update.install(bytes).map_err(|e| {
            format!(
                "Windows 更新安装失败: {e}。已执行退出前清理，代理或 Live 接管可能已暂停；请重启应用或重新开启代理后再试。"
            )
        })?;
        Ok(true)
    }

    #[cfg(not(target_os = "windows"))]
    {
        // macOS/Linux install() 会返回；先安装，避免安装失败时误停代理/撤回接管。
        update
            .install(bytes)
            .map_err(|e| format!("安装更新失败: {e}"))?;

        crate::save_window_state_before_exit(&app);
        crate::cleanup_before_exit(&app).await;

        log::info!("应用更新安装完成，正在重启应用");
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        crate::restart_process(&app);
    }
}

/// 检查是否有可用的应用更新，返回可用的新版本号（无更新时返回 None）。
///
/// 数据库版本过新的恢复界面用它判断：升级应用能否解决问题。若返回 None，说明
/// 已是最新版本，但数据库仍不兼容（通常由第三方客户端或更高版本创建），应提示用户
/// 升级无法解决，而不是让其反复尝试。
#[tauri::command]
pub async fn check_app_update_available(app: AppHandle) -> Result<Option<String>, String> {
    let updater = app
        .updater_builder()
        .build()
        .map_err(|e| format!("初始化更新器失败: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("检查更新失败: {e}"))?;
    Ok(update.map(|u| u.version))
}

/// 获取 app_config_dir 覆盖配置 (从 Store)
#[tauri::command]
pub async fn get_app_config_dir_override(app: AppHandle) -> Result<Option<String>, String> {
    Ok(crate::app_store::refresh_app_config_dir_override(&app)
        .map(|p| p.to_string_lossy().to_string()))
}

/// 设置 app_config_dir 覆盖配置 (到 Store)
#[tauri::command]
pub async fn set_app_config_dir_override(
    app: AppHandle,
    path: Option<String>,
) -> Result<bool, String> {
    crate::app_store::set_app_config_dir_to_store(&app, path.as_deref())?;
    Ok(true)
}

/// 设置开机自启
#[tauri::command]
pub async fn set_auto_launch(enabled: bool) -> Result<bool, String> {
    if enabled {
        crate::auto_launch::enable_auto_launch().map_err(|e| format!("启用开机自启失败: {e}"))?;
    } else {
        crate::auto_launch::disable_auto_launch().map_err(|e| format!("禁用开机自启失败: {e}"))?;
    }
    Ok(true)
}


/// 获取开机自启状态
#[tauri::command]
pub async fn get_auto_launch_status() -> Result<bool, String> {
    crate::auto_launch::is_auto_launch_enabled().map_err(|e| format!("获取开机自启状态失败: {e}"))
}

/// 获取整流器配置
#[tauri::command]
pub async fn get_rectifier_config(
    state: tauri::State<'_, crate::AppState>,
) -> Result<crate::proxy::types::RectifierConfig, String> {
    state.db.get_rectifier_config().map_err(|e| e.to_string())
}

/// 设置整流器配置
#[tauri::command]
pub async fn set_rectifier_config(
    state: tauri::State<'_, crate::AppState>,
    config: crate::proxy::types::RectifierConfig,
) -> Result<bool, String> {
    state
        .db
        .set_rectifier_config(&config)
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// 获取优化器配置
#[tauri::command]
pub async fn get_optimizer_config(
    state: tauri::State<'_, crate::AppState>,
) -> Result<crate::proxy::types::OptimizerConfig, String> {
    state.db.get_optimizer_config().map_err(|e| e.to_string())
}

/// 设置优化器配置
#[tauri::command]
pub async fn set_optimizer_config(
    state: tauri::State<'_, crate::AppState>,
    config: crate::proxy::types::OptimizerConfig,
) -> Result<bool, String> {
    state
        .db
        .set_optimizer_config(&config)
        .map_err(|e| e.to_string())?;
    Ok(true)
}

/// 获取日志配置
#[tauri::command]
pub async fn get_log_config(
    state: tauri::State<'_, crate::AppState>,
) -> Result<crate::proxy::types::LogConfig, String> {
    state.db.get_log_config().map_err(|e| e.to_string())
}

/// 设置日志配置
#[tauri::command]
pub async fn set_log_config(
    state: tauri::State<'_, crate::AppState>,
    config: crate::proxy::types::LogConfig,
) -> Result<bool, String> {
    state
        .db
        .set_log_config(&config)
        .map_err(|e| e.to_string())?;
    log::set_max_level(config.to_level_filter());
    log::info!(
        "日志配置已更新: enabled={}, level={}",
        config.enabled,
        config.level
    );
    Ok(true)
}

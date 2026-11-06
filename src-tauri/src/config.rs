use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::AppError;

/// 获取用户主目录，带回退和日志
///
/// ## Windows 注意事项
///
/// - `dirs::home_dir()` 在 Windows 上使用 `SHGetKnownFolderPath(FOLDERID_Profile)`，
///   返回的是真实用户目录（类似 `C:\\Users\\Alice`），与 v3.10.2 行为一致。
/// - 不要直接使用 `HOME` 环境变量：它可能由 Git/Cygwin/MSYS 等第三方工具注入，
///   且不一定等于用户目录，可能导致 `.cc-switch/cc-switch.db` 路径变化，从而“看起来像数据丢失”。
///
/// ## 测试隔离
///
/// 为了让 Windows CI/本地测试能稳定隔离真实用户数据，可通过 `CC_SWITCH_TEST_HOME`
/// 显式覆盖 home dir（仅用于测试/调试场景）。
pub fn get_home_dir() -> PathBuf {
    if let Some(home) = test_home_override() {
        return home;
    }

    dirs::home_dir().unwrap_or_else(|| {
        log::warn!("无法获取用户主目录，回退到当前目录");
        PathBuf::from(".")
    })
}

/// 检测路径是否为 WSL 网络路径（如 \\wsl$\Ubuntu\... 或 \\wsl.localhost\Ubuntu\...）
/// 注意：仅检测直接 UNC 路径，映射磁盘符（如 Z: -> \\wsl$\...）无法检测
#[cfg(windows)]
pub(crate) fn is_wsl_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        match prefix.kind() {
            Prefix::UNC(server, _) | Prefix::VerbatimUNC(server, _) => {
                let s = server.to_string_lossy();
                s.eq_ignore_ascii_case("wsl$") || s.eq_ignore_ascii_case("wsl.localhost")
            }
            _ => false,
        }
    } else {
        false
    }
}

/// `CC_SWITCH_TEST_HOME` 的覆盖值（测试/调试用的 home 覆盖）。
///
/// 返回 `Some` 即表示显式覆盖生效——此时任何基于“库里有没有 db”的启发式回退
/// 都不该再介入，否则测试会被引导到真实用户数据上。
///
/// 用 `var_os` 而非 `var`：非 Unicode 的取值只应让路径变 lossy，而不该让
/// “覆盖是否存在”的判断失效、进而退回真实用户目录。
fn test_home_override() -> Option<PathBuf> {
    let raw = std::env::var_os("CC_SWITCH_TEST_HOME")?;
    // to_string_lossy 的结果必须先绑住，否则 trim 借的是一个已释放的临时值。
    let lossy = raw.to_string_lossy();
    let trimmed = lossy.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// 获取 Claude Code 配置目录路径
pub fn get_claude_config_dir() -> PathBuf {
    if let Some(custom) = crate::settings::get_claude_override_dir() {
        return custom;
    }

    get_home_dir().join(".claude")
}

/// 获取 Claude Code 主配置文件路径
pub fn get_claude_settings_path() -> PathBuf {
    let dir = get_claude_config_dir();
    let settings = dir.join("settings.json");
    if settings.exists() {
        return settings;
    }
    // 兼容旧版命名：若存在旧文件则继续使用
    let legacy = dir.join("claude.json");
    if legacy.exists() {
        return legacy;
    }
    // 默认新建：回落到标准文件名 settings.json（不再生成 claude.json）
    settings
}

/// 获取应用配置目录路径 (~/.ca-switch)
pub fn get_app_config_dir() -> PathBuf {
    if let Some(custom) = crate::app_store::get_app_config_dir_override() {
        return custom;
    }

    let default_dir = get_home_dir().join(".ca-switch");

    // P5：从旧品牌数据目录（~/.cc-switch）一次性复制数据库。
    // 只在新目录还没有自己的数据库时执行，避免覆盖已经用起来的新库。
    migrate_legacy_config_dir(&default_dir, &get_home_dir().join(".cc-switch"));

    // 兼容 v3.10.3：当用户环境存在 `HOME` 且与真实用户目录不同，
    // v3.10.3 可能在 `HOME/.cc-switch/` 下创建/使用了数据库。
    // 这里仅在“默认位置没有数据库”时回退到旧位置，避免再次出现“供应商消失”问题，
    // 同时也避免新安装因为 `HOME` 被设置而写入非预期路径。
    //
    // `CC_SWITCH_TEST_HOME` 是测试用的显式 home 覆盖，必须在这里短路：测试的临时
    // 目录本来就不带 cc-switch.db，若不先返回，下面的 HOME 回退会把测试指向真实
    // 用户库（Windows 上 runner 的 HOME 下常常确实有该 db），既污染用户数据，又让
    // 隔离测试读到别人写的状态而失败。
    #[cfg(windows)]
    if test_home_override().is_some() {
        return default_dir;
    }

    #[cfg(windows)]
    {
        let default_db = default_dir.join("ca-switch.db");
        if !default_db.exists() {
            if let Ok(home_env) = std::env::var("HOME") {
                let trimmed = home_env.trim();
                if !trimmed.is_empty() {
                    let legacy_home = PathBuf::from(trimmed);
                    for legacy_dir in [legacy_home.join(".cc-switch"), legacy_home.join(".ca-switch")] {
                        if legacy_dir.join("cc-switch.db").exists()
                            || legacy_dir.join("ca-switch.db").exists()
                        {
                            log::info!(
                                "Detected legacy database at {}, copying it into {}",
                                legacy_dir.display(),
                                default_dir.display()
                            );
                            migrate_legacy_config_dir(&default_dir, &legacy_dir);
                            break;
                        }
                    }
                }
            }
        }
    }

    default_dir
}

/// 把旧品牌目录里的数据库复制到新目录。
///
/// 只在两个条件同时成立时动作：新目录没有数据库、旧目录有数据库。
/// 复制失败只记录，不阻塞启动（最坏情况是用户看到空列表，而不是应用起不来）。
fn migrate_legacy_config_dir(new_dir: &Path, legacy_dir: &Path) {
    if new_dir.join("ca-switch.db").exists() || !legacy_dir.is_dir() {
        return;
    }
    let legacy_db = if legacy_dir.join("ca-switch.db").exists() {
        legacy_dir.join("ca-switch.db")
    } else {
        legacy_dir.join("cc-switch.db")
    };
    if !legacy_db.exists() {
        return;
    }
    if let Err(error) = std::fs::create_dir_all(new_dir) {
        eprintln!(
            "创建配置目录失败 ({}): {error}",
            new_dir.display()
        );
        return;
    }
    match std::fs::copy(&legacy_db, new_dir.join("ca-switch.db")) {
        Ok(_) => eprintln!(
            "已从旧数据目录迁移数据库: {} -> {}",
            legacy_db.display(),
            new_dir.join("ca-switch.db").display()
        ),
        Err(error) => eprintln!(
            "迁移旧数据库失败 ({}): {error}",
            legacy_db.display()
        ),
    }
}

/// 获取应用配置文件路径
pub fn get_app_config_path() -> PathBuf {
    get_app_config_dir().join("config.json")
}

/// 清理供应商名称，确保文件名安全
#[allow(dead_code)]
pub fn sanitize_provider_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            _ => c,
        })
        .collect::<String>()
        .to_lowercase()
}

/// 获取供应商配置文件路径
#[allow(dead_code)]
pub fn get_provider_config_path(provider_id: &str, provider_name: Option<&str>) -> PathBuf {
    let base_name = provider_name
        .map(sanitize_provider_name)
        .unwrap_or_else(|| sanitize_provider_name(provider_id));

    get_claude_config_dir().join(format!("settings-{base_name}.json"))
}

/// 读取 JSON 配置文件
pub fn read_json_file<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<T, AppError> {
    if !path.exists() {
        return Err(AppError::Config(format!("文件不存在: {}", path.display())));
    }

    let content = fs::read_to_string(path).map_err(|e| AppError::io(path, e))?;

    serde_json::from_str(&content).map_err(|e| AppError::json(path, e))
}

/// 递归排序 JSON 对象的键（按字母顺序），确保序列化输出是确定性的
fn sort_json_keys(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted_map = Map::new();
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for key in keys {
                sorted_map.insert(key.clone(), sort_json_keys(&map[key]));
            }
            Value::Object(sorted_map)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_json_keys).collect()),
        other => other.clone(),
    }
}

/// 写入 JSON 配置文件并返回实际写入的字节。
pub fn write_json_file_with_contents<T: Serialize>(
    path: &Path,
    data: &T,
) -> Result<Vec<u8>, AppError> {
    // 确保目录存在
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let contents = sorted_json_bytes(data)?;
    atomic_write(path, &contents)?;
    Ok(contents)
}

pub(crate) fn sorted_json_bytes<T: Serialize>(data: &T) -> Result<Vec<u8>, AppError> {
    let value = serde_json::to_value(data).map_err(|e| AppError::JsonSerialize { source: e })?;
    let sorted_value = sort_json_keys(&value);
    let json = serde_json::to_string_pretty(&sorted_value)
        .map_err(|e| AppError::JsonSerialize { source: e })?;
    Ok(json.into_bytes())
}

/// 写入 JSON 配置文件（键按字母排序，确保确定性输出）
pub fn write_json_file<T: Serialize>(path: &Path, data: &T) -> Result<(), AppError> {
    write_json_file_with_contents(path, data).map(|_| ())
}

/// 原子写入：写入临时文件后 rename 替换，避免半写状态
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<(), AppError> {
    atomic_write_with_unix_mode(path, data, None)
}

/// 原子写入包含凭据的文件。Unix 上新文件和替换文件始终使用 0600。
pub fn atomic_write_private(path: &Path, data: &[u8]) -> Result<(), AppError> {
    atomic_write_with_unix_mode(path, data, Some(0o600))
}

fn atomic_write_with_unix_mode(
    path: &Path,
    data: &[u8],
    unix_mode: Option<u32>,
) -> Result<(), AppError> {
    stage_write(path, data, unix_mode, false)?.commit()
}

/// 已写好、还没替换目标的临时文件。原子写的前半步：写入引擎先把一次操作涉及的
/// 所有文件都备好临时文件、记下写前意图，再逐个 [`StagedWrite::commit`]。
#[derive(Debug)]
pub struct StagedWrite {
    tmp: PathBuf,
    path: PathBuf,
}

impl StagedWrite {
    /// 用临时文件替换目标（失败时删掉临时文件）。
    pub fn commit(self) -> Result<(), AppError> {
        commit_staged(&self.tmp, &self.path).inspect_err(|_| {
            let _ = fs::remove_file(&self.tmp);
        })
    }
}

/// 写入临时文件：Unix 下 `unix_mode` 为 `None` 时沿用目标文件现有的权限位。
/// `durable` 为真时写完先 fsync，崩溃恢复要靠这份临时文件前滚。
pub(crate) fn stage_write(
    path: &Path,
    data: &[u8],
    unix_mode: Option<u32>,
    durable: bool,
) -> Result<StagedWrite, AppError> {
    #[cfg(not(unix))]
    let _ = unix_mode;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
    }

    let parent = path
        .parent()
        .ok_or_else(|| AppError::Config("无效的路径".to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Config("无效的文件名".to_string()))?
        .to_string_lossy()
        .to_string();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (tmp, mut file) = (|| -> Result<(PathBuf, fs::File), AppError> {
        let mut last_collision = None;
        for _ in 0..16 {
            let counter = TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let candidate = parent.join(format!(
                "{file_name}.tmp.{}.{ts}.{counter}",
                std::process::id()
            ));
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            if let Some(mode) = unix_mode {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }
            match options.open(&candidate) {
                Ok(file) => return Ok((candidate, file)),
                Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_collision = Some((candidate, source));
                }
                Err(source) => return Err(AppError::io(&candidate, source)),
            }
        }

        let (candidate, source) = last_collision.expect("temporary filename loop must run");
        Err(AppError::io(&candidate, source))
    })()?;

    let written = file
        .write_all(data)
        .and_then(|_| file.flush())
        .and_then(|_| if durable { file.sync_all() } else { Ok(()) });
    if let Err(source) = written {
        drop(file);
        let _ = fs::remove_file(&tmp);
        return Err(AppError::io(&tmp, source));
    }
    drop(file);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = unix_mode {
            if let Err(source) = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode)) {
                let _ = fs::remove_file(&tmp);
                return Err(AppError::io(&tmp, source));
            }
        } else if let Ok(meta) = fs::metadata(path) {
            let perm = meta.permissions().mode();
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(perm));
        }
    }

    Ok(StagedWrite {
        tmp,
        path: path.to_path_buf(),
    })
}

/// 原子写的后半步：用 `tmp` 替换 `path`。崩溃恢复也用它提交上次留下的临时文件。
///
/// 失败时临时文件留在原处：写入引擎的 pending 指着它，下次恢复要靠它前滚（目标文件被
/// 占用、只读这类失败，过后多半能补完）。只做一次性原子写的调用方自己删。
pub(crate) fn commit_staged(tmp: &Path, path: &Path) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::ERROR_NOT_SUPPORTED, Storage::FileSystem::ReplaceFileW,
        };

        // WSL UNC paths reject ReplaceFileW with ERROR_NOT_SUPPORTED (50) on
        // every attempt; treat it as failed up front and let the rename
        // fallback below do the work, instead of burning the syscall (and the
        // retry loop) on each write.
        let skip_replace = is_wsl_path(path);

        let replaced: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let replacement: Vec<u16> = tmp
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut completed = false;
        let mut last_error = None;

        for _ in 0..3 {
            // SAFETY: both path buffers are NUL-terminated UTF-16 and remain alive for the
            // duration of the call. Backup, exclusion, and reserved pointers are intentionally null.
            let replaced_ok = if skip_replace {
                0
            } else {
                unsafe {
                    ReplaceFileW(
                        replaced.as_ptr(),
                        replacement.as_ptr(),
                        std::ptr::null(),
                        0,
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                }
            };
            if replaced_ok != 0 {
                completed = true;
                break;
            }

            let replace_error = std::io::Error::last_os_error();
            // WSL UNC paths reject ReplaceFileW with ERROR_NOT_SUPPORTED (50).
            // std::fs::rename uses a different replace-existing API on Windows.
            let replace_not_supported =
                skip_replace || replace_error.raw_os_error() == Some(ERROR_NOT_SUPPORTED as i32);
            if replace_error.kind() != std::io::ErrorKind::NotFound && !replace_not_supported {
                last_error = Some(replace_error);
                break;
            }

            match fs::rename(tmp, path) {
                Ok(()) => {
                    completed = true;
                    break;
                }
                Err(source)
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    last_error = Some(source);
                }
                Err(source) => {
                    last_error = Some(source);
                    break;
                }
            }
        }

        if !completed {
            let source = last_error.unwrap_or_else(std::io::Error::last_os_error);
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }
    }

    #[cfg(not(windows))]
    {
        if let Err(source) = fs::rename(tmp, path) {
            return Err(AppError::IoContext {
                context: format!("原子替换失败: {} -> {}", tmp.display(), path.display()),
                source,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 共享的环境变量互斥锁：改 `HOME` / `CC_SWITCH_TEST_HOME` 这类进程级
    /// 状态的测试都必须经它（配合 `#[serial_test::serial]`）。锁必须放在
    /// 函数里经 `OnceLock` 取用——写成某个测试函数体内的 `static` 只对那一个
    /// 测试可见，等于没有互斥。
    ///
    /// 目前唯一使用者是下面的 Windows 回归测试，故同样 cfg 掉，避免在
    /// Linux/macOS 的 `clippy -D warnings` 里变成 never-used。
    #[cfg(windows)]
    fn env_lock() -> &'static std::sync::Mutex<()> {
        use std::sync::OnceLock;
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    fn assert_atomic_write_replaces_existing_file(dir: &Path) {
        let path = dir.join("atomic-write-contract.json");
        std::fs::write(&path, b"old contents").unwrap();

        atomic_write(&path, b"new contents").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new contents");
        let tmp_prefix = "atomic-write-contract.json.tmp.";
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(tmp_prefix))
            .map(|entry| entry.path())
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remain: {leftovers:?}"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_atomic_write_replaces_existing_file(dir.path());
    }


    #[cfg(windows)]
    #[test]
    fn atomic_write_preserves_destination_when_windows_replace_fails() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, b"old contents").unwrap();
        let held_file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();

        let result = atomic_write(&path, b"new contents");

        assert!(result.is_err());
        drop(held_file);
        assert_eq!(std::fs::read(&path).unwrap(), b"old contents");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR to point to a WSL2 UNC directory"]
    fn atomic_write_replaces_existing_wsl_unc_file() {
        let root = PathBuf::from(
            std::env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let home = get_home_dir();
        let temp = std::env::temp_dir();
        for (name, path) in [
            ("test root", root.as_path()),
            ("test home", home.as_path()),
            ("temporary directory", temp.as_path()),
        ] {
            let unc = path.to_string_lossy();
            assert!(
                unc.starts_with(r"\\wsl.localhost\") || unc.starts_with(r"\\wsl$\"),
                "expected {name} to be a WSL UNC path, got {unc}"
            );
            assert!(
                path.starts_with(&root),
                "expected {name} to be under {}, got {unc}",
                root.display()
            );
        }

        let dir = tempfile::Builder::new()
            .prefix("atomic-write-contract-")
            .tempdir_in(&root)
            .unwrap();
        assert_atomic_write_replaces_existing_file(dir.path());
    }


    #[test]
    fn sort_json_keys_sorts_top_level_object() {
        let input = serde_json::json!({
            "z": 1,
            "a": 2,
            "m": 3,
        });
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn sort_json_keys_recurses_into_nested_objects() {
        let input = serde_json::json!({
            "outer_b": {"z": 1, "a": 2},
            "outer_a": {"y": 3, "b": 4},
        });
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(
            serialized,
            r#"{"outer_a":{"b":4,"y":3},"outer_b":{"a":2,"z":1}}"#
        );
    }

    #[test]
    fn sort_json_keys_preserves_array_order() {
        let input = serde_json::json!([3, 1, 2]);
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, "[3,1,2]");
    }

    #[test]
    fn sort_json_keys_sorts_objects_inside_arrays_but_keeps_array_order() {
        let input = serde_json::json!([
            {"z": 1, "a": 2},
            {"y": 3, "b": 4},
        ]);
        let sorted = sort_json_keys(&input);
        let serialized = serde_json::to_string(&sorted).unwrap();
        assert_eq!(serialized, r#"[{"a":2,"z":1},{"b":4,"y":3}]"#);
    }

    #[test]
    fn sort_json_keys_passes_through_primitives() {
        let cases = vec![
            serde_json::json!("hello"),
            serde_json::json!(42),
            serde_json::json!(3.5),
            serde_json::json!(true),
            serde_json::json!(null),
        ];
        for value in cases {
            let sorted = sort_json_keys(&value);
            assert_eq!(sorted, value);
        }
    }

    #[test]
    fn sort_json_keys_handles_empty_collections() {
        let empty_obj = serde_json::json!({});
        assert_eq!(
            serde_json::to_string(&sort_json_keys(&empty_obj)).unwrap(),
            "{}"
        );

        let empty_arr = serde_json::json!([]);
        assert_eq!(
            serde_json::to_string(&sort_json_keys(&empty_arr)).unwrap(),
            "[]"
        );
    }

    #[test]
    fn sort_json_keys_produces_identical_output_for_different_insertion_orders() {
        // 核心保证：同一逻辑配置无论键的插入顺序如何，写出的字节序列必须一致。
        let mut a = Map::new();
        a.insert("env".to_string(), serde_json::json!({"PATH": "/usr/bin"}));
        a.insert("model".to_string(), serde_json::json!("claude-sonnet-4-5"));
        a.insert("permissions".to_string(), serde_json::json!({"allow": []}));

        let mut b = Map::new();
        b.insert("permissions".to_string(), serde_json::json!({"allow": []}));
        b.insert("model".to_string(), serde_json::json!("claude-sonnet-4-5"));
        b.insert("env".to_string(), serde_json::json!({"PATH": "/usr/bin"}));

        let sorted_a = sort_json_keys(&Value::Object(a));
        let sorted_b = sort_json_keys(&Value::Object(b));

        assert_eq!(
            serde_json::to_string(&sorted_a).unwrap(),
            serde_json::to_string(&sorted_b).unwrap(),
        );
    }

    /// 回归：`CC_SWITCH_TEST_HOME` 必须盖过 v3.10.3 的 `HOME` 兼容回退。
    ///
    /// Windows 上回退只看"默认位置有没有 cc-switch.db"，而测试临时目录里通常没有，
    /// 于是会被引向 `HOME/.cc-switch`——runner 的 HOME 下往往真有一份 db，测试就此
    /// 读写到真实用户数据（既污染用户库，又让断言读到别人写下的状态）。
    #[cfg(windows)]
    #[test]
    #[serial_test::serial]
    fn test_home_short_circuits_the_home_legacy_fallback() {
        // 进程级环境变量的读写必须与同样改它们的测试互斥。锁是模块级共享的
        // （写成测试函数体内的 static 只能锁住自己，等于没锁），见
        // `proxy::http_client` 里同一套 env_lock() 用法。
        let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());

        // 假装 HOME 指向另一处，并且那里有一份 db——正是会触发回退的形状
        let legacy_home = tempfile::tempdir().expect("tempdir");
        let legacy_dir = legacy_home.path().join(".ca-switch");
        std::fs::create_dir_all(&legacy_dir).expect("create legacy dir");
        std::fs::write(legacy_dir.join("ca-switch.db"), b"legacy").expect("seed legacy db");

        let test_home = tempfile::tempdir().expect("tempdir");
        let saved_home = std::env::var_os("HOME");
        let saved_test_home = std::env::var_os("CC_SWITCH_TEST_HOME");

        std::env::set_var("HOME", legacy_home.path());
        std::env::set_var("CC_SWITCH_TEST_HOME", test_home.path());

        let dir = get_app_config_dir();

        match saved_test_home {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
        match saved_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }

        let expected = test_home.path().join(".ca-switch");
        assert_eq!(
            dir, expected,
            "测试 home 优先：不得回退到 HOME 下的旧库目录"
        );
    }
}

/// 复制文件
pub fn copy_file(from: &Path, to: &Path) -> Result<(), AppError> {
    fs::copy(from, to).map_err(|e| AppError::IoContext {
        context: format!("复制文件失败 ({} -> {})", from.display(), to.display()),
        source: e,
    })?;
    Ok(())
}


/// 检查 Claude Code 配置状态
#[derive(Serialize, Deserialize)]
pub struct ConfigStatus {
    pub exists: bool,
    pub path: String,
}

/// 获取 Claude Code 配置状态
pub fn get_claude_config_status() -> ConfigStatus {
    let path = get_claude_settings_path();
    ConfigStatus {
        exists: path.exists(),
        path: path.to_string_lossy().to_string(),
    }
}

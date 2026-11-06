#![allow(non_snake_case)]

mod balance;
mod coding_plan;
mod config;
mod custom_icon;
mod deeplink;
mod env;
mod failover;
mod global_proxy;
mod import_export;
mod misc;
mod model_fetch;
mod profile;
mod provider;
mod proxy;
mod settings;
mod stream_check;
mod subscription;
mod sync_support;

mod lightweight;
mod s3_sync;
mod usage;
mod webdav_sync;

pub use balance::*;
pub use coding_plan::*;
pub use config::*;
pub use custom_icon::*;
pub use deeplink::*;
pub use env::*;
pub use failover::*;
pub use global_proxy::*;
pub use import_export::*;
pub use misc::*;
pub use model_fetch::*;
pub use profile::*;
pub use provider::*;
pub use proxy::*;
pub use settings::*;
pub use stream_check::*;
pub use subscription::*;

pub use lightweight::*;
pub use s3_sync::*;
pub use usage::*;
pub use webdav_sync::*;

#[cfg(not(target_os = "windows"))]
#[allow(unused_imports)] // 仅被 `#[cfg(test)]` 下的工具探测用例使用
pub(crate) use misc::{login_shell_path, merge_path_segments, CommandDeadline};

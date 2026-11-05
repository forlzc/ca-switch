#![allow(non_snake_case)]

mod agy_account;
mod auth;
mod balance;
mod claude_account;
mod codex_oauth;
mod coding_plan;
mod config;
mod copilot;
mod custom_icon;
mod deeplink;
mod dsh;
mod env;
mod failover;
mod global_proxy;
mod hermes;
mod import_export;
mod kimi_oauth;
mod misc;
mod model_fetch;
mod omo;
mod omp;
mod openclaw;
mod pi;
mod plugin;
mod profile;
mod provider;
mod proxy;
mod settings;
mod stream_check;
mod subscription;
mod sync_support;
mod xai_oauth;

mod lightweight;
mod s3_sync;
mod usage;
mod webdav_sync;

#[cfg(test)]
mod provider_concurrency_tests;
#[cfg(test)]
mod startup_tests;

pub use agy_account::*;
pub use auth::*;
pub use balance::*;
pub use claude_account::*;
pub use codex_oauth::*;
pub use coding_plan::*;
pub use config::*;
pub use copilot::*;
pub use custom_icon::*;
pub use deeplink::*;
pub use dsh::*;
pub use env::*;
pub use failover::*;
pub use global_proxy::*;
pub use hermes::*;
pub use import_export::*;
pub use kimi_oauth::*;
pub use misc::*;
pub use model_fetch::*;
pub use omo::*;
pub(crate) use omp::*;
pub use openclaw::*;
pub(crate) use pi::*;
pub use plugin::*;
pub use profile::*;
pub use provider::*;
pub use proxy::*;
pub use settings::*;
pub use stream_check::*;
pub use subscription::*;
pub use xai_oauth::*;

pub use lightweight::*;
pub use s3_sync::*;
pub use usage::*;
pub use webdav_sync::*;

#[cfg(not(target_os = "windows"))]
#[allow(unused_imports)] // 仅被 `#[cfg(test)]` 下的工具探测用例使用
pub(crate) use misc::{login_shell_path, merge_path_segments, CommandDeadline};

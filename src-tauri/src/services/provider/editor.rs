//! 供应商编辑器的显示与保存载荷。
//!
//! caswitch 不再把供应商投影进客户端配置文件，编辑器里看到的就是这一行自己的配置：
//! 显示什么由 `settings_config` 决定，切到这家之后客户端长什么样不再由这里推算。

use serde::Deserialize;
use serde_json::Value;

/// 编辑器保存时一并送来的上下文。caswitch 只认 `settings_config`，这些字段为兼容旧前端保留。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorSave {
    #[serde(default)]
    pub base: Option<Value>,
    #[serde(default)]
    pub draft: Option<Value>,
    #[serde(default)]
    pub on_conflict: ConflictPolicy,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    #[default]
    Refuse,
    KeepMine,
    KeepTheirs,
}


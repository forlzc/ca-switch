//! 供应商编辑器的显示与保存载荷。
//!
//! caswitch 不再把供应商投影进客户端配置文件，编辑器里看到的就是这一行自己的配置：
//! 显示什么由 `settings_config` 决定，切到这家之后客户端长什么样不再由这里推算。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 编辑器里显示的完整配置。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorView {
    /// 编辑器里显示的配置。
    pub settings: Value,
    /// 行里保存着、但不随切换生效的字段。没有了 live 之后恒为空，字段保留以免前端改动。
    pub inactive: Vec<InactiveField>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InactiveField {
    /// 键路径，比如 `["env", "API_TIMEOUT_MS"]`。
    pub path: Vec<String>,
    pub value: Value,
}

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

/// 编辑器显示的内容：这张卡自己的配置。
pub fn view(settings_config: &Value) -> EditorView {
    EditorView {
        settings: settings_config.clone(),
        inactive: Vec::new(),
    }
}

//! OpenAI Chat Completions ↔ Anthropic Messages 直连转换。
//!
//! 只在「Chat 入口 × Anthropic 上游」这一格使用（`Matrix::plan` 的
//! `OpenAiChat → Anthropic`）。请求方向把 Chat 请求体转成 Anthropic Messages，
//! 响应方向把 Anthropic 的 JSON / SSE 转回 Chat Completions。
//!
//! 覆盖文本、图片、工具调用（含历史 tool_calls / tool 消息）、system/developer
//! 指令、温度与输出上限。Anthropic 独有的 thinking 历史不做强造，上游默认关闭。

use crate::proxy::sse::{strip_sse_field, SseBuffer};
use crate::proxy::ProxyError;
use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Anthropic 要求 max_tokens；Chat 请求缺省时使用这个保守上限。
const DEFAULT_MAX_TOKENS: u64 = 8192;

/// Chat Completions 请求体 → Anthropic Messages 请求体。
pub fn chat_request_to_anthropic(chat: Value) -> Result<Value, ProxyError> {
    let mut out = json!({});

    if let Some(model) = chat.get("model").and_then(Value::as_str) {
        out["model"] = json!(model);
    }

    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();

    if let Some(items) = chat.get("messages").and_then(Value::as_array) {
        for message in items {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user");
            match role {
                "system" | "developer" => {
                    let text = chat_text_content(message.get("content"));
                    if !text.is_empty() {
                        system_parts.push(text);
                    }
                }
                "assistant" => {
                    let mut blocks = chat_content_to_anthropic_blocks(message.get("content"));
                    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array)
                    {
                        for (i, call) in tool_calls.iter().enumerate() {
                            let name = call
                                .pointer("/function/name")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let arguments = call
                                .pointer("/function/arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}");
                            let id = call
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("call_{i}"));
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": parse_arguments(arguments),
                            }));
                        }
                    }
                    push_anthropic_message(&mut messages, "assistant", blocks);
                }
                "tool" => {
                    let tool_use_id = message
                        .get("tool_call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let content = message
                        .get("content")
                        .map(|content| chat_text_content(Some(content)))
                        .unwrap_or_default();
                    push_anthropic_message(
                        &mut messages,
                        "user",
                        vec![json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                        })],
                    );
                }
                _ => {
                    let blocks = chat_content_to_anthropic_blocks(message.get("content"));
                    push_anthropic_message(&mut messages, "user", blocks);
                }
            }
        }
    }

    if !system_parts.is_empty() {
        out["system"] = json!(system_parts.join("\n\n"));
    }
    if messages.is_empty() {
        return Err(ProxyError::InvalidRequest(
            "cannot convert Chat request: empty messages".to_string(),
        ));
    }
    ensure_leading_user_message(&mut messages);
    out["messages"] = json!(messages);

    let max_tokens = chat
        .get("max_completion_tokens")
        .or_else(|| chat.get("max_tokens"))
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    out["max_tokens"] = json!(max_tokens);

    for key in ["temperature", "top_p", "stream"] {
        if let Some(value) = chat.get(key) {
            out[key] = value.clone();
        }
    }
    if let Some(stop) = chat.get("stop") {
        let sequences = match stop {
            Value::String(text) => vec![json!(text)],
            Value::Array(items) => items.clone(),
            _ => Vec::new(),
        };
        if !sequences.is_empty() {
            out["stop_sequences"] = json!(sequences);
        }
    }
    if let Some(user) = chat.get("user").and_then(Value::as_str) {
        out["metadata"] = json!({ "user_id": user });
    }

    let tools = chat
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(chat_tool_to_anthropic_tool)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let tool_choice_is_none = matches!(
        chat.get("tool_choice").and_then(Value::as_str),
        Some("none")
    );
    if !tools.is_empty() && !tool_choice_is_none {
        out["tools"] = json!(tools);
    }
    if let Some(choice) = chat.get("tool_choice") {
        if let Some(converted) = chat_tool_choice_to_anthropic(choice) {
            out["tool_choice"] = converted;
        }
    }

    Ok(out)
}

fn push_anthropic_message(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some(role) {
            if let Some(content) = last.get_mut("content").and_then(Value::as_array_mut) {
                content.extend(blocks);
                return;
            }
        }
    }
    messages.push(json!({ "role": role, "content": blocks }));
}

/// Anthropic 要求首条消息是 user；Chat 历史偶尔以 assistant 开头。
fn ensure_leading_user_message(messages: &mut Vec<Value>) {
    let leads_with_user = messages
        .first()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("user");
    if !messages.is_empty() && !leads_with_user {
        messages.insert(
            0,
            json!({
                "role": "user",
                "content": [{ "type": "text", "text": "(continuing the conversation)" }]
            }),
        );
    }
}

fn parse_arguments(arguments: &str) -> Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| json!({}))
}

fn chat_text_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("text");
                match part_type {
                    "text" | "input_text" | "output_text" => {
                        part.get("text").and_then(Value::as_str).map(str::to_string)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Chat 的 content（字符串或 parts 数组）→ Anthropic content blocks。
fn chat_content_to_anthropic_blocks(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) if !text.is_empty() => {
            vec![json!({ "type": "text", "text": text })]
        }
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(chat_part_to_anthropic_block)
            .collect(),
        _ => Vec::new(),
    }
}

fn chat_part_to_anthropic_block(part: &Value) -> Option<Value> {
    match part.get("type").and_then(Value::as_str).unwrap_or("text") {
        "text" | "input_text" | "output_text" => part
            .get("text")
            .and_then(Value::as_str)
            .map(|text| json!({ "type": "text", "text": text })),
        "image_url" => {
            let url = part.pointer("/image_url/url").and_then(Value::as_str)?;
            Some(image_url_to_anthropic_block(url))
        }
        _ => None,
    }
}

fn image_url_to_anthropic_block(url: &str) -> Value {
    // data URL：拆成 base64 + media_type，Anthropic 原生支持。
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((meta, data)) = rest.split_once(',') {
            if let Some(media_type) = meta.strip_suffix(";base64") {
                return json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }
                });
            }
        }
    }
    json!({
        "type": "image",
        "source": { "type": "url", "url": url }
    })
}

fn chat_tool_to_anthropic_tool(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        return None;
    }
    let function = tool.get("function")?;
    let name = function.get("name").and_then(Value::as_str)?;
    let mut converted = json!({
        "name": name,
        "input_schema": function
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
    });
    if let Some(description) = function.get("description") {
        converted["description"] = description.clone();
    }
    Some(converted)
}

fn chat_tool_choice_to_anthropic(choice: &Value) -> Option<Value> {
    match choice {
        Value::String(mode) => match mode.as_str() {
            "auto" => Some(json!({ "type": "auto" })),
            "required" | "any" => Some(json!({ "type": "any" })),
            // Anthropic 没有「禁止工具」的 tool_choice；由上层直接不发 tools。
            "none" => None,
            _ => None,
        },
        Value::Object(_) => {
            let name = choice
                .pointer("/function/name")
                .or_else(|| choice.pointer("/name"))
                .and_then(Value::as_str)?;
            Some(json!({ "type": "tool", "name": name }))
        }
        _ => None,
    }
}

/// Anthropic Messages 非流式响应 → Chat Completions 响应。
pub fn anthropic_response_to_chat(response: &Value, fallback_model: Option<&str>) -> Value {
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-converted");
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
        .or(fallback_model)
        .unwrap_or("unknown");

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    if let Some(blocks) = response.get("content").and_then(Value::as_array) {
        for block in blocks {
            match block.get("type").and_then(Value::as_str).unwrap_or("") {
                "text" => {
                    if let Some(chunk) = block.get("text").and_then(Value::as_str) {
                        text.push_str(chunk);
                    }
                }
                "thinking" => {
                    if let Some(chunk) = block.get("thinking").and_then(Value::as_str) {
                        reasoning.push_str(chunk);
                    }
                }
                "tool_use" => {
                    let arguments = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    tool_calls.push(json!({
                        "id": block.get("id").and_then(Value::as_str).unwrap_or(""),
                        "type": "function",
                        "function": {
                            "name": block.get("name").and_then(Value::as_str).unwrap_or(""),
                            "arguments": serde_json::to_string(&arguments)
                                .unwrap_or_else(|_| "{}".to_string()),
                        }
                    }));
                }
                _ => {}
            }
        }
    }

    let stop_reason = response
        .get("stop_reason")
        .and_then(Value::as_str)
        .unwrap_or("end_turn");
    let finish_reason = map_finish_reason(stop_reason, !tool_calls.is_empty());

    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { json!(text) },
    });
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }

    let mut out = json!({
        "id": id,
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp() as u64,
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
    });
    if let Some(usage) = response.get("usage") {
        out["usage"] = anthropic_usage_to_chat(usage);
    }
    out
}

/// Anthropic 错误体 → Chat Completions 错误体。
pub fn anthropic_error_to_chat(status: u16, body: &Value) -> Value {
    let message = body
        .pointer("/error/message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("upstream anthropic request failed");
    let error_type = body
        .pointer("/error/type")
        .and_then(Value::as_str)
        .unwrap_or("upstream_error");
    json!({
        "error": {
            "message": message,
            "type": error_type,
            "code": status,
        }
    })
}

fn anthropic_usage_to_chat(usage: &Value) -> Value {
    let input = token_count(usage, "input_tokens")
        + token_count(usage, "cache_creation_input_tokens")
        + token_count(usage, "cache_read_input_tokens");
    let output = token_count(usage, "output_tokens");
    json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": input + output,
    })
}

fn token_count(usage: &Value, key: &str) -> u64 {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn map_finish_reason(stop_reason: &str, has_tool_calls: bool) -> &'static str {
    if has_tool_calls {
        return "tool_calls";
    }
    match stop_reason {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        "end_turn" | "stop_sequence" | "pause_turn" => "stop",
        _ => "stop",
    }
}

#[derive(Default)]
struct ChatStreamState {
    id: String,
    model: String,
    created: u64,
    started: bool,
    completed: bool,
    finish_reason: Option<String>,
    prompt_tokens: u64,
    completion_tokens: u64,
    saw_usage: bool,
    block_tool_indices: HashMap<u64, usize>,
    next_tool_index: usize,
    saw_tool: bool,
}

impl ChatStreamState {
    fn ensure_metadata(&mut self, message: Option<&Value>) {
        if let Some(message) = message {
            if let Some(id) = message.get("id").and_then(Value::as_str) {
                self.id = id.to_string();
            }
            if let Some(model) = message.get("model").and_then(Value::as_str) {
                self.model = model.to_string();
            }
            if let Some(usage) = message.get("usage") {
                self.capture_usage(usage);
            }
        }
        if self.id.is_empty() {
            self.id = "chatcmpl-converted".to_string();
        }
        if self.created == 0 {
            self.created = chrono::Utc::now().timestamp() as u64;
        }
    }

    fn capture_usage(&mut self, usage: &Value) {
        self.saw_usage = true;
        let input = token_count(usage, "input_tokens")
            + token_count(usage, "cache_creation_input_tokens")
            + token_count(usage, "cache_read_input_tokens");
        if input > 0 {
            self.prompt_tokens = input;
        }
        let output = token_count(usage, "output_tokens");
        if output > 0 {
            self.completion_tokens = output;
        }
    }

    fn start_if_needed(&mut self) -> Vec<Bytes> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        vec![self.chunk(json!({ "role": "assistant", "content": "" }), None)]
    }

    fn handle(&mut self, name: &str, value: &Value) -> Vec<Bytes> {
        match name {
            "message_start" => {
                self.ensure_metadata(value.get("message"));
                self.start_if_needed()
            }
            "content_block_start" => {
                let block = value.get("content_block").unwrap_or(&Value::Null);
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    return Vec::new();
                }
                self.saw_tool = true;
                let block_index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
                let index = self.next_tool_index;
                self.next_tool_index += 1;
                self.block_tool_indices.insert(block_index, index);
                let mut events = self.start_if_needed();
                events.push(self.chunk(
                    json!({
                        "tool_calls": [{
                            "index": index,
                            "id": block.get("id").and_then(Value::as_str).unwrap_or(""),
                            "type": "function",
                            "function": {
                                "name": block.get("name").and_then(Value::as_str).unwrap_or(""),
                                "arguments": "",
                            }
                        }]
                    }),
                    None,
                ));
                events
            }
            "content_block_delta" => {
                let delta = value.get("delta").unwrap_or(&Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        if text.is_empty() {
                            return Vec::new();
                        }
                        let mut events = self.start_if_needed();
                        events.push(self.chunk(json!({ "content": text }), None));
                        events
                    }
                    "thinking_delta" => {
                        let thinking = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if thinking.is_empty() {
                            return Vec::new();
                        }
                        let mut events = self.start_if_needed();
                        events.push(self.chunk(json!({ "reasoning_content": thinking }), None));
                        events
                    }
                    "input_json_delta" => {
                        let partial = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if partial.is_empty() {
                            return Vec::new();
                        }
                        let block_index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let index = self.block_tool_indices.get(&block_index).copied().unwrap_or(0);
                        let mut events = self.start_if_needed();
                        events.push(self.chunk(
                            json!({
                                "tool_calls": [{ "index": index, "function": { "arguments": partial } }]
                            }),
                            None,
                        ));
                        events
                    }
                    _ => Vec::new(),
                }
            }
            "message_delta" => {
                if let Some(delta) = value.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                        self.finish_reason = Some(map_finish_reason(reason, self.saw_tool).to_string());
                    }
                }
                if let Some(usage) = value.get("usage") {
                    self.capture_usage(usage);
                }
                Vec::new()
            }
            "message_stop" => {
                let events = self.finalize();
                self.completed = true;
                events
            }
            "error" => {
                let message = value
                    .pointer("/error/message")
                    .or_else(|| value.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("upstream response failed");
                self.completed = true;
                vec![Bytes::from(format!(
                    "data: {}\n\n",
                    json!({ "error": { "message": message, "type": "upstream_error" } })
                ))]
            }
            _ => Vec::new(),
        }
    }

    fn finalize(&mut self) -> Vec<Bytes> {
        let mut events = Vec::new();
        if !self.started {
            events.extend(self.start_if_needed());
        }
        let reason = self.finish_reason.clone().unwrap_or_else(|| {
            if self.saw_tool {
                "tool_calls"
            } else {
                "stop"
            }
            .to_string()
        });
        let mut body = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": reason,
            }],
        });
        if self.saw_usage {
            body["usage"] = json!({
                "prompt_tokens": self.prompt_tokens,
                "completion_tokens": self.completion_tokens,
                "total_tokens": self.prompt_tokens + self.completion_tokens,
            });
        }
        events.push(Bytes::from(format!("data: {body}\n\n")));
        events.push(Bytes::from("data: [DONE]\n\n"));
        self.completed = true;
        events
    }

    fn chunk(&self, delta: Value, finish_reason: Option<&str>) -> Bytes {
        let body = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason,
            }],
        });
        Bytes::from(format!("data: {body}\n\n"))
    }
}

/// Anthropic SSE → Chat Completions SSE。
pub fn create_chat_sse_stream_from_anthropic<E>(
    stream: impl Stream<Item = Result<Bytes, E>> + Send + 'static,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    E: std::error::Error + Send + 'static,
{
    async_stream::stream! {
        let mut sse = SseBuffer::new();
        let mut state = ChatStreamState::default();
        let mut finished = false;

        tokio::pin!(stream);

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    sse.push_bytes(&bytes);
                    while let Some(block) = sse.next_block() {
                        if block.trim().is_empty() {
                            continue;
                        }

                        let mut event_name: Option<String> = None;
                        let mut data_parts: Vec<String> = Vec::new();
                        for line in block.lines() {
                            if let Some(event) = strip_sse_field(line, "event") {
                                event_name = Some(event.trim().to_string());
                            }
                            if let Some(data) = strip_sse_field(line, "data") {
                                data_parts.push(data.to_string());
                            }
                        }
                        if data_parts.is_empty() {
                            continue;
                        }
                        let data = data_parts.join("\n");
                        if data.trim() == "[DONE]" {
                            for event in state.finalize() {
                                yield Ok(event);
                            }
                            return;
                        }
                        let Ok(value) = serde_json::from_str::<Value>(&data) else {
                            continue;
                        };
                        let name = event_name
                            .clone()
                            .or_else(|| value.get("type").and_then(Value::as_str).map(str::to_string))
                            .unwrap_or_default();

                        for event in state.handle(&name, &value) {
                            yield Ok(event);
                        }
                        if state.completed {
                            finished = true;
                            break;
                        }
                    }
                    if finished {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        if !finished {
            for event in state.finalize() {
                yield Ok(event);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_request_converts_system_tools_and_tool_history() {
        let chat = json!({
            "model": "bailu-2.7",
            "max_tokens": 256,
            "temperature": 0.3,
            "stop": ["\n\n"],
            "messages": [
                {"role": "system", "content": "你是助手"},
                {"role": "user", "content": "查一下天气"},
                {"role": "assistant", "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "weather", "arguments": "{\"city\":\"上海\"}"}
                }]},
                {"role": "tool", "tool_call_id": "call_1", "content": "晴 25 度"},
                {"role": "user", "content": [
                    {"type": "text", "text": "谢谢"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
                ]}
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "weather",
                    "description": "查询天气",
                    "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
                }
            }],
            "tool_choice": "auto",
            "stream": false
        });

        let converted = chat_request_to_anthropic(chat).expect("convert");
        assert_eq!(converted["model"], "bailu-2.7");
        assert_eq!(converted["max_tokens"], 256);
        assert_eq!(converted["temperature"], 0.3);
        assert_eq!(converted["stop_sequences"], json!(["\n\n"]));
        assert_eq!(converted["system"], "你是助手");
        assert_eq!(converted["tools"][0]["name"], "weather");
        assert_eq!(converted["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(converted["tool_choice"], json!({"type": "auto"}));

        let messages = converted["messages"].as_array().expect("messages");
        // 连续的同角色消息会合并：tool_result 与后续 user 文本/图片落在同一条 user 消息。
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["input"]["city"], "上海");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(messages[2]["content"][1]["type"], "text");
        assert_eq!(messages[2]["content"][2]["type"], "image");
        assert_eq!(messages[2]["content"][2]["source"]["type"], "base64");
    }

    #[test]
    fn chat_request_merges_consecutive_same_role_turns() {
        let converted = chat_request_to_anthropic(json!({
            "model": "m",
            "messages": [
                {"role": "user", "content": "a"},
                {"role": "user", "content": "b"}
            ]
        }))
        .expect("convert");
        let messages = converted["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn anthropic_response_converts_text_tools_and_usage() {
        let response = json!({
            "id": "msg_1",
            "model": "bailu-2.7",
            "content": [
                {"type": "text", "text": "OK"},
                {"type": "tool_use", "id": "toolu_1", "name": "weather", "input": {"city": "上海"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "cache_read_input_tokens": 5, "output_tokens": 20}
        });
        let converted = anthropic_response_to_chat(&response, None);
        assert_eq!(converted["object"], "chat.completion");
        assert_eq!(converted["choices"][0]["message"]["content"], "OK");
        assert_eq!(converted["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(converted["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "weather");
        assert_eq!(converted["usage"]["prompt_tokens"], 15);
        assert_eq!(converted["usage"]["completion_tokens"], 20);
        assert_eq!(converted["usage"]["total_tokens"], 35);
    }

    #[test]
    fn anthropic_finish_reason_mapping() {
        assert_eq!(map_finish_reason("end_turn", false), "stop");
        assert_eq!(map_finish_reason("max_tokens", false), "length");
        assert_eq!(map_finish_reason("tool_use", false), "tool_calls");
        assert_eq!(map_finish_reason("stop_sequence", false), "stop");
    }

    #[tokio::test]
    async fn anthropic_sse_converts_to_chat_sse() {
        let events = [
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"bailu-2.7\",\"usage\":{\"input_tokens\":10}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"OK\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ];
        let bytes: Vec<Result<Bytes, std::io::Error>> =
            events.iter().map(|e| Ok(Bytes::from(e.to_string()))).collect();
        let stream = futures::stream::iter(bytes);
        let output: Vec<Bytes> = create_chat_sse_stream_from_anthropic(stream)
            .collect::<Vec<Result<Bytes, std::io::Error>>>()
            .await
            .into_iter()
            .filter_map(Result::ok)
            .collect();
        let text = output
            .iter()
            .map(|b| String::from_utf8_lossy(b).to_string())
            .collect::<String>();
        assert!(text.contains("chat.completion.chunk"));
        assert!(text.contains("\"content\":\"OK\""));
        assert!(text.contains("\"finish_reason\":\"stop\""));
        assert!(text.contains("\"prompt_tokens\":10"));
        assert!(text.contains("data: [DONE]"));
    }
}

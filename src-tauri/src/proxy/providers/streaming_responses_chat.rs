//! OpenAI Responses → OpenAI Chat Completions 的响应方向转换。
//!
//! 与 `transform_chat_responses.rs` 成对：请求方向把 Chat 请求转成 Responses，
//! 这里把 Responses 的 JSON / SSE 响应转回 Chat 形状。
//!
//! 覆盖文本、推理摘要、函数调用与 usage；`response.failed` 归一成 Chat 的
//! `{"error": {...}}` 分块。

use crate::proxy::sse::{strip_sse_field, SseBuffer};
use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Responses 非流式响应 → Chat Completions 响应。
pub fn responses_response_to_chat(response: &Value, fallback_model: Option<&str>) -> Value {
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
    let created = response
        .get("created_at")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| chrono::Utc::now().timestamp() as u64);

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();

    if let Some(output) = response.get("output").and_then(Value::as_array) {
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    if let Some(parts) = item.get("content").and_then(Value::as_array) {
                        for part in parts {
                            if matches!(
                                part.get("type").and_then(Value::as_str),
                                Some("output_text") | Some("text")
                            ) {
                                if let Some(chunk) = part.get("text").and_then(Value::as_str) {
                                    text.push_str(chunk);
                                }
                            }
                        }
                    }
                }
                Some("reasoning") => {
                    for key in ["summary", "content"] {
                        if let Some(parts) = item.get(key).and_then(Value::as_array) {
                            for part in parts {
                                if let Some(chunk) = part.get("text").and_then(Value::as_str) {
                                    reasoning.push_str(chunk);
                                }
                            }
                        }
                    }
                }
                Some("function_call") => {
                    tool_calls.push(json!({
                        "id": item.get("call_id").and_then(Value::as_str).unwrap_or(""),
                        "type": "function",
                        "function": {
                            "name": item.get("name").and_then(Value::as_str).unwrap_or(""),
                            "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                        }
                    }));
                }
                _ => {}
            }
        }
    }

    let finish_reason = finish_reason_for(response, !tool_calls.is_empty());
    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() { Value::Null } else { json!(text) },
    });
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }

    json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": response.get("usage").map(responses_usage_to_chat),
    })
}

fn finish_reason_for(response: &Value, has_tool_calls: bool) -> &'static str {
    match response.get("status").and_then(Value::as_str) {
        Some("incomplete") => "length",
        Some("failed") => "stop",
        _ if has_tool_calls => "tool_calls",
        _ => "stop",
    }
}

/// Responses usage → Chat usage。字段缺失时保持缺省，避免伪造 0。
fn responses_usage_to_chat(usage: &Value) -> Value {
    let input = usage.get("input_tokens").cloned().unwrap_or(Value::Null);
    let output = usage.get("output_tokens").cloned().unwrap_or(Value::Null);
    let total = usage
        .get("total_tokens")
        .cloned()
        .unwrap_or_else(|| json_sum(&input, &output));
    let mut converted = json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": total,
    });
    if let Some(cached) = usage.pointer("/input_tokens_details/cached_tokens") {
        converted["prompt_tokens_details"] = json!({ "cached_tokens": cached });
    }
    converted
}

fn json_sum(a: &Value, b: &Value) -> Value {
    match (a.as_u64(), b.as_u64()) {
        (Some(a), Some(b)) => json!(a + b),
        _ => Value::Null,
    }
}

/// Responses SSE → Chat SSE。
pub fn create_chat_sse_stream_from_responses<E>(
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

        'outer: loop {
            let Some(chunk) = stream.next().await else {
                break 'outer;
            };
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
                            break 'outer;
                        }
                    }
                }
                Err(_) => break 'outer,
            }
        }

        if !finished {
            for event in state.finalize() {
                yield Ok(event);
            }
        }
    }
}

#[derive(Default)]
struct ChatStreamState {
    id: String,
    model: String,
    created: u64,
    started: bool,
    completed: bool,
    saw_tool: bool,
    finish_reason: Option<String>,
    usage: Option<Value>,
    /// Responses function_call item id → Chat tool_calls 的 index。
    tool_indices: HashMap<String, usize>,
    next_tool_index: usize,
}

impl ChatStreamState {
    fn ensure_metadata(&mut self, response: Option<&Value>) {
        if let Some(response) = response {
            if let Some(id) = response.get("id").and_then(Value::as_str) {
                self.id = id.to_string();
            }
            if let Some(model) = response.get("model").and_then(Value::as_str) {
                self.model = model.to_string();
            }
            if let Some(created) = response.get("created_at").and_then(Value::as_u64) {
                self.created = created;
            }
            if let Some(usage) = response.get("usage") {
                self.usage = Some(usage.clone());
            }
        }
        if self.id.is_empty() {
            self.id = "chatcmpl-converted".to_string();
        }
        if self.created == 0 {
            self.created = chrono::Utc::now().timestamp() as u64;
        }
    }

    fn start_if_needed(&mut self) -> Vec<Bytes> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        vec![self.chunk(
            json!({ "role": "assistant", "content": "" }),
            None,
        )]
    }

    fn handle(&mut self, name: &str, value: &Value) -> Vec<Bytes> {
        match name {
            "response.created" | "response.in_progress" => {
                self.ensure_metadata(value.get("response"));
                self.start_if_needed()
            }
            "response.output_text.delta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if delta.is_empty() {
                    return Vec::new();
                }
                let mut events = self.start_if_needed();
                events.push(self.chunk(json!({ "content": delta }), None));
                events
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if delta.is_empty() {
                    return Vec::new();
                }
                let mut events = self.start_if_needed();
                events.push(self.chunk(json!({ "reasoning_content": delta }), None));
                events
            }
            "response.output_item.added" => {
                let item = value.get("item").unwrap_or(&Value::Null);
                if item.get("type").and_then(Value::as_str) != Some("function_call") {
                    return Vec::new();
                }
                self.saw_tool = true;
                let key = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let index = self.next_tool_index;
                self.next_tool_index += 1;
                self.tool_indices.insert(key, index);
                let mut events = self.start_if_needed();
                events.push(self.chunk(
                    json!({
                        "tool_calls": [{
                            "index": index,
                            "id": item.get("call_id").and_then(Value::as_str).unwrap_or(""),
                            "type": "function",
                            "function": {
                                "name": item.get("name").and_then(Value::as_str).unwrap_or(""),
                                "arguments": "",
                            }
                        }]
                    }),
                    None,
                ));
                events
            }
            "response.function_call_arguments.delta" => {
                let delta = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if delta.is_empty() {
                    return Vec::new();
                }
                let key = value
                    .get("item_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let index = *self.tool_indices.get(key).unwrap_or(&0);
                let mut events = self.start_if_needed();
                events.push(self.chunk(
                    json!({
                        "tool_calls": [{ "index": index, "function": { "arguments": delta } }]
                    }),
                    None,
                ));
                events
            }
            "response.completed" | "response.incomplete" => {
                self.ensure_metadata(value.get("response"));
                self.completed = true;
                let reason = if name == "response.incomplete" {
                    "length".to_string()
                } else if self.saw_tool {
                    "tool_calls".to_string()
                } else {
                    "stop".to_string()
                };
                self.finish_reason = Some(reason);
                self.finalize()
            }
            "response.failed" | "error" => {
                let message = value
                    .pointer("/response/error/message")
                    .or_else(|| value.pointer("/error/message"))
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
        let reason = self
            .finish_reason
            .clone()
            .unwrap_or_else(|| {
                if self.saw_tool {
                    "tool_calls"
                } else {
                    "stop"
                }
                .to_string()
            });
        let mut chunk = self.chunk(json!({}), Some(reason.as_str()));
        if let Some(usage) = &self.usage {
            chunk_json_mut(&mut chunk, "usage", responses_usage_to_chat(usage));
        }
        events.push(chunk);
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

/// `chunk` 是已经序列化好的 SSE 字节，这里重新解析以追加字段。
fn chunk_json_mut(chunk: &mut Bytes, key: &str, value: Value) {
    let text = String::from_utf8_lossy(chunk).to_string();
    let Some(data) = text
        .lines()
        .find_map(|line| strip_sse_field(line, "data"))
    else {
        return;
    };
    let Ok(mut parsed) = serde_json::from_str::<Value>(data) else {
        return;
    };
    parsed[key] = value;
    *chunk = Bytes::from(format!("data: {parsed}\n\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use std::time::Duration;

    async fn collect(chunks: Vec<&str>) -> String {
        let chunks: Vec<Result<Bytes, std::io::Error>> = chunks
            .into_iter()
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk.as_bytes())))
            .collect();
        let upstream = stream::iter(chunks);
        let converted = create_chat_sse_stream_from_responses(upstream);
        let bytes: Vec<Bytes> = converted.map(|item| item.unwrap()).collect().await;
        String::from_utf8(bytes.concat()).unwrap()
    }

    fn events(output: &str) -> Vec<Value> {
        output
            .split("\n\n")
            .filter_map(|block| {
                let data = block.lines().find_map(|line| strip_sse_field(line, "data"))?;
                if data.trim() == "[DONE]" {
                    return None;
                }
                serde_json::from_str(data).ok()
            })
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_upstream_completes_without_synthetic_chunks() {
        // 上游在首个事件后静默 60s（推理阶段）：转换器只等真实事件，不补空分块
        let upstream = stream::unfold(0u8, |step| async move {
            let chunk = match step {
                0 => Some("event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_hb\",\"model\":\"gpt-5.4\",\"created_at\":123}}\n\n"),
                1 => {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Some("event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n")
                }
                2 => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Some("event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_hb\",\"status\":\"completed\"}}\n\n")
                }
                _ => None,
            }?;
            Some((Ok::<Bytes, std::io::Error>(Bytes::from(chunk)), step + 1))
        });

        let converted = create_chat_sse_stream_from_responses(upstream);
        let output = converted
            .map(|item| item.unwrap())
            .collect::<Vec<_>>()
            .await;
        let output = String::from_utf8(output.concat()).unwrap();

        let events = events(&output);
        let empty_delta_chunks = events
            .iter()
            .filter(|event| {
                event["choices"][0]["delta"]
                    .as_object()
                    .is_some_and(|delta| delta.is_empty())
                    && event["choices"][0]["finish_reason"].is_null()
            })
            .count();
        assert_eq!(
            empty_delta_chunks, 0,
            "静默期不应补空 delta 分块，实际补了 {empty_delta_chunks} 个"
        );
        assert!(events.iter().any(|event| {
            event["choices"][0]["finish_reason"].as_str() == Some("stop")
        }));
        assert!(output.contains("data: [DONE]"));
    }

    #[test]
    fn converts_non_stream_response_with_tool_call() {
        let response = json!({
            "id": "resp_1",
            "model": "gpt-5.4",
            "created_at": 123,
            "status": "completed",
            "output": [
                { "type": "message", "role": "assistant",
                  "content": [{ "type": "output_text", "text": "let me check" }] },
                { "type": "function_call", "call_id": "call_1", "name": "get_weather",
                  "arguments": "{\"city\":\"SH\"}" }
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "total_tokens": 15,
                "input_tokens_details": { "cached_tokens": 2 }
            }
        });
        let chat = responses_response_to_chat(&response, None);
        assert_eq!(chat["object"], "chat.completion");
        assert_eq!(chat["id"], "resp_1");
        assert_eq!(chat["choices"][0]["message"]["content"], "let me check");
        assert_eq!(chat["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            chat["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
        assert_eq!(chat["usage"]["prompt_tokens"], 10);
        assert_eq!(chat["usage"]["completion_tokens"], 5);
        assert_eq!(chat["usage"]["total_tokens"], 15);
        assert_eq!(chat["usage"]["prompt_tokens_details"]["cached_tokens"], 2);
    }

    #[tokio::test]
    async fn converts_streaming_text_and_completion() {
        let output = collect(vec![
            "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-5.4\",\"created_at\":123}}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-5.4\",\"usage\":{\"input_tokens\":4,\"output_tokens\":2,\"total_tokens\":6}}}\n\n",
        ])
        .await;
        let events = events(&output);
        assert_eq!(events[0]["choices"][0]["delta"]["role"], "assistant");
        let text: String = events
            .iter()
            .filter_map(|event| event["choices"][0]["delta"]["content"].as_str())
            .collect();
        assert_eq!(text, "Hello");
        let final_event = events.last().unwrap();
        assert_eq!(final_event["choices"][0]["finish_reason"], "stop");
        assert_eq!(final_event["usage"]["prompt_tokens"], 4);
        assert!(output.contains("data: [DONE]"));
    }

    #[tokio::test]
    async fn converts_streaming_tool_call() {
        let output = collect(vec![
            "event: response.output_item.added\ndata: {\"type\":\"response.output_item.added\",\"item\":{\"id\":\"item_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"get_weather\"}}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"item_1\",\"delta\":\"{\\\"city\\\":\"}\n\n",
            "event: response.function_call_arguments.delta\ndata: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"item_1\",\"delta\":\"\\\"SH\\\"}\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"model\":\"gpt-5.4\"}}\n\n",
        ])
        .await;
        let events = events(&output);
        let tool_delta = events
            .iter()
            .find(|event| event["choices"][0]["delta"]["tool_calls"].is_array())
            .unwrap();
        assert_eq!(tool_delta["choices"][0]["delta"]["tool_calls"][0]["id"], "call_1");
        let args: String = events
            .iter()
            .filter_map(|event| {
                event["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
                    .as_str()
            })
            .collect();
        assert_eq!(args, "{\"city\":\"SH\"}");
        assert_eq!(
            events.last().unwrap()["choices"][0]["finish_reason"],
            "tool_calls"
        );
    }
}

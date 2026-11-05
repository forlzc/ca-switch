//! OpenAI Chat Completions → OpenAI Responses 的请求方向转换。
//!
//! 只在「Chat 入口 × Responses 上游」这一格使用（`Matrix::plan` 的
//! `OpenAiChat → OpenAiResponses`）。响应方向见 `streaming_responses_chat.rs`。
//!
//! 覆盖范围：文本、图片、工具调用（含历史 tool_calls / tool 消息）、
//! `response_format`、温度与输出上限。Responses 独有的 reasoning / store 等
//! 字段不强造，保持上游默认行为。

use crate::proxy::ProxyError;
use serde_json::{json, Value};

/// 把 Chat Completions 请求体转成 Responses 请求体。
pub fn chat_request_to_responses(chat: Value) -> Result<Value, ProxyError> {
    let mut out = json!({});

    if let Some(model) = chat.get("model") {
        out["model"] = model.clone();
    }

    let mut instructions = String::new();
    let mut input: Vec<Value> = Vec::new();

    if let Some(messages) = chat.get("messages").and_then(Value::as_array) {
        for message in messages {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user");
            match role {
                "system" | "developer" => {
                    let text = message_text(message.get("content"));
                    if !text.is_empty() {
                        if !instructions.is_empty() {
                            instructions.push_str("\n\n");
                        }
                        instructions.push_str(&text);
                    }
                }
                "assistant" => {
                    let text = message_text(message.get("content"));
                    if !text.is_empty() {
                        input.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{ "type": "output_text", "text": text }]
                        }));
                    }
                    if let Some(tool_calls) =
                        message.get("tool_calls").and_then(Value::as_array)
                    {
                        for call in tool_calls {
                            let name = call
                                .pointer("/function/name")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let arguments = call
                                .pointer("/function/arguments")
                                .and_then(Value::as_str)
                                .unwrap_or("{}");
                            input.push(json!({
                                "type": "function_call",
                                "call_id": call.get("id").and_then(Value::as_str).unwrap_or(""),
                                "name": name,
                                "arguments": arguments,
                            }));
                        }
                    }
                }
                "tool" => {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": message
                            .get("tool_call_id")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                        "output": message_text(message.get("content")),
                    }));
                }
                _ => {
                    input.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": content_parts(message.get("content"), false),
                    }));
                }
            }
        }
    }

    if !instructions.is_empty() {
        out["instructions"] = json!(instructions);
    }
    out["input"] = json!(input);

    if let Some(max_output) = chat
        .get("max_completion_tokens")
        .or_else(|| chat.get("max_tokens"))
    {
        out["max_output_tokens"] = max_output.clone();
    }
    for key in [
        "temperature",
        "top_p",
        "stream",
        "parallel_tool_calls",
        "user",
        "metadata",
    ] {
        if let Some(value) = chat.get(key) {
            out[key] = value.clone();
        }
    }

    if let Some(tools) = chat.get("tools").and_then(Value::as_array) {
        let converted: Vec<Value> = tools
            .iter()
            .filter_map(chat_tool_to_responses_tool)
            .collect();
        if !converted.is_empty() {
            out["tools"] = json!(converted);
        }
    }

    if let Some(tool_choice) = chat.get("tool_choice") {
        out["tool_choice"] = chat_tool_choice_to_responses(tool_choice);
    }

    if let Some(response_format) = chat.get("response_format") {
        if let Some(format) = chat_response_format_to_responses(response_format) {
            out["text"] = json!({ "format": format });
        }
    }

    Ok(out)
}

fn chat_tool_to_responses_tool(tool: &Value) -> Option<Value> {
    if tool.get("type").and_then(Value::as_str) != Some("function") {
        // Responses 的内置工具（web_search / file_search 等）不在这条链路上强转。
        return None;
    }
    let function = tool.get("function")?;
    let name = function.get("name").and_then(Value::as_str)?;
    let mut converted = json!({
        "type": "function",
        "name": name,
    });
    for key in ["description", "parameters", "strict"] {
        if let Some(value) = function.get(key) {
            converted[key] = value.clone();
        }
    }
    Some(converted)
}

fn chat_tool_choice_to_responses(choice: &Value) -> Value {
    match choice {
        Value::String(mode) => json!(mode),
        Value::Object(_) => {
            let name = choice
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("");
            json!({ "type": "function", "name": name })
        }
        other => other.clone(),
    }
}

fn chat_response_format_to_responses(format: &Value) -> Option<Value> {
    match format.get("type").and_then(Value::as_str) {
        Some("json_schema") => {
            let schema = format.get("json_schema")?;
            let mut converted = json!({ "type": "json_schema" });
            for key in ["name", "schema", "strict"] {
                if let Some(value) = schema.get(key) {
                    converted[key] = value.clone();
                }
            }
            Some(converted)
        }
        Some("json_object") => Some(json!({ "type": "json_object" })),
        Some("text") => Some(json!({ "type": "text" })),
        _ => None,
    }
}

fn message_text(content: Option<&Value>) -> String {
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

/// Chat 的 content（字符串或 parts 数组）→ Responses 的 content parts。
fn content_parts(content: Option<&Value>, assistant: bool) -> Value {
    let text_type = if assistant { "output_text" } else { "input_text" };
    match content {
        Some(Value::String(text)) => json!([{ "type": text_type, "text": text }]),
        Some(Value::Array(parts)) => {
            let mut out = Vec::new();
            for part in parts {
                match part.get("type").and_then(Value::as_str).unwrap_or("text") {
                    "text" | "input_text" | "output_text" => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            out.push(json!({ "type": text_type, "text": text }));
                        }
                    }
                    "image_url" => {
                        if let Some(url) = part.pointer("/image_url/url").and_then(Value::as_str)
                        {
                            out.push(json!({ "type": "input_image", "image_url": url }));
                        }
                    }
                    _ => {}
                }
            }
            json!(out)
        }
        _ => json!([]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_text_and_system_messages() {
        let chat = json!({
            "model": "gpt-5.4",
            "messages": [
                { "role": "system", "content": "You are terse." },
                { "role": "user", "content": "hi" }
            ],
            "max_tokens": 128,
            "temperature": 0.2,
            "stream": true,
            "stream_options": { "include_usage": true }
        });
        let responses = chat_request_to_responses(chat).unwrap();
        assert_eq!(responses["model"], "gpt-5.4");
        assert_eq!(responses["instructions"], "You are terse.");
        assert_eq!(responses["max_output_tokens"], 128);
        assert_eq!(responses["temperature"], 0.2);
        assert_eq!(responses["stream"], true);
        assert!(responses.get("stream_options").is_none());
        assert_eq!(responses["input"][0]["type"], "message");
        assert_eq!(responses["input"][0]["role"], "user");
        assert_eq!(responses["input"][0]["content"][0]["type"], "input_text");
    }

    #[test]
    fn converts_tool_history_and_definitions() {
        let chat = json!({
            "model": "gpt-5.4",
            "messages": [
                { "role": "user", "content": "weather?" },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "get_weather", "arguments": "{\"city\":\"SH\"}" }
                    }]
                },
                { "role": "tool", "tool_call_id": "call_1", "content": "sunny" }
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "get_weather",
                    "description": "Get weather",
                    "parameters": { "type": "object", "properties": { "city": { "type": "string" } } }
                }
            }],
            "tool_choice": { "type": "function", "function": { "name": "get_weather" } }
        });
        let responses = chat_request_to_responses(chat).unwrap();
        assert_eq!(responses["tools"][0]["type"], "function");
        assert_eq!(responses["tools"][0]["name"], "get_weather");
        assert_eq!(responses["tools"][0]["parameters"]["type"], "object");
        assert_eq!(responses["tool_choice"]["type"], "function");
        assert_eq!(responses["tool_choice"]["name"], "get_weather");

        let items = responses["input"].as_array().unwrap();
        let call = items
            .iter()
            .find(|item| item["type"] == "function_call")
            .unwrap();
        assert_eq!(call["call_id"], "call_1");
        assert_eq!(call["name"], "get_weather");
        assert_eq!(call["arguments"], "{\"city\":\"SH\"}");
        let output = items
            .iter()
            .find(|item| item["type"] == "function_call_output")
            .unwrap();
        assert_eq!(output["call_id"], "call_1");
        assert_eq!(output["output"], "sunny");
    }

    #[test]
    fn converts_image_parts() {
        let chat = json!({
            "model": "gpt-5.4",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "what is this" },
                    { "type": "image_url", "image_url": { "url": "https://example.com/a.png" } }
                ]
            }]
        });
        let responses = chat_request_to_responses(chat).unwrap();
        let parts = responses["input"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[1]["type"], "input_image");
        assert_eq!(parts[1]["image_url"], "https://example.com/a.png");
    }

    #[test]
    fn converts_json_schema_response_format() {
        let chat = json!({
            "model": "gpt-5.4",
            "messages": [{ "role": "user", "content": "x" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "answer",
                    "strict": true,
                    "schema": { "type": "object" }
                }
            }
        });
        let responses = chat_request_to_responses(chat).unwrap();
        assert_eq!(responses["text"]["format"]["type"], "json_schema");
        assert_eq!(responses["text"]["format"]["name"], "answer");
        assert_eq!(responses["text"]["format"]["strict"], true);
    }
}

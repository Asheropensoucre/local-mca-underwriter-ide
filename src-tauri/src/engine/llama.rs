//! Minimal client for llama-server's OpenAI-compatible chat endpoint.
//!
//! Streaming is always on so the UI can show reasoning and tokens live; the
//! stream is decoded from server-sent events with a proper line buffer (chunks
//! may split or join events arbitrarily).

use super::runtime::Endpoint;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

/// One chat message. `image_data_uri`, when set, is attached as an OpenAI-style
/// `image_url` part in front of the text.
pub struct Message<'a> {
    pub role: &'a str,
    pub text: &'a str,
    pub image_data_uri: Option<&'a str>,
}

pub struct ChatOptions<'a> {
    pub model: &'a str,
    pub temperature: f32,
    pub max_tokens: i32,
    /// JSON schema the output must satisfy (llama-server compiles it to a grammar).
    pub json_schema: Option<Value>,
    /// Qwen3-style thinking. Off for extraction (it costs minutes on small GPUs).
    pub enable_thinking: bool,
    /// No response activity for this long aborts the request.
    pub idle_timeout: Duration,
}

/// Result of a chat call. `reasoning` is the model's thinking text when the
/// template exposes it as `reasoning_content`.
#[derive(Debug, Default, Serialize)]
pub struct ChatResult {
    pub content: String,
    pub reasoning: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// Stream a chat completion. `on_delta(reasoning_delta, content_delta)` is called
/// for every token so callers can forward them as UI events.
pub async fn chat(
    ep: &Endpoint,
    messages: &[Message<'_>],
    opts: &ChatOptions<'_>,
    mut on_delta: impl FnMut(Option<&str>, Option<&str>),
) -> Result<ChatResult, String> {
    let msgs: Vec<Value> = messages
        .iter()
        .map(|m| match m.image_data_uri {
            Some(uri) => json!({
                "role": m.role,
                "content": [
                    { "type": "image_url", "image_url": { "url": uri } },
                    { "type": "text", "text": m.text }
                ]
            }),
            None => json!({ "role": m.role, "content": m.text }),
        })
        .collect();

    let mut body = json!({
        "model": opts.model,
        "messages": msgs,
        "temperature": opts.temperature,
        "max_tokens": opts.max_tokens,
        "stream": true,
        "stream_options": { "include_usage": true },
        "chat_template_kwargs": { "enable_thinking": opts.enable_thinking },
    });
    if let Some(schema) = &opts.json_schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": { "name": "output", "schema": schema }
        });
    }

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(format!("{}/v1/chat/completions", ep.base_url))
        .bearer_auth(&ep.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Engine request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Engine returned {status}: {}", text.chars().take(500).collect::<String>()));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut result = ChatResult::default();
    let mut finished = false;

    while !finished {
        let next = tokio::time::timeout(opts.idle_timeout, stream.next()).await;
        let chunk = match next {
            Err(_) => return Err(format!("Engine produced no output for {} seconds", opts.idle_timeout.as_secs())),
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(format!("Engine stream error: {e}")),
            Ok(Some(Ok(bytes))) => bytes,
        };
        buf.extend_from_slice(&chunk);

        // SSE events are separated by a blank line. Process every complete event.
        while let Some(pos) = find_event_end(&buf) {
            let event: Vec<u8> = buf.drain(..pos.end).collect();
            let event = String::from_utf8_lossy(&event[..pos.start]).to_string();
            for line in event.lines() {
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    finished = true;
                    break;
                }
                let v: Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(err) = v.get("error") {
                    return Err(format!("Engine error: {err}"));
                }
                if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
                    result.prompt_tokens = usage.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                    result.completion_tokens = usage.get("completion_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                }
                let delta = v.pointer("/choices/0/delta");
                let reasoning = delta.and_then(|d| d.get("reasoning_content")).and_then(|x| x.as_str());
                let content = delta.and_then(|d| d.get("content")).and_then(|x| x.as_str());
                if reasoning.is_some() || content.is_some() {
                    if let Some(r) = reasoning {
                        result.reasoning.push_str(r);
                    }
                    if let Some(c) = content {
                        result.content.push_str(c);
                    }
                    on_delta(reasoning, content);
                }
            }
        }
    }
    Ok(result)
}

/// Locate the end of the first complete SSE event in `buf`: returns the byte range
/// of the event body (start) and where the next event begins (end).
fn find_event_end(buf: &[u8]) -> Option<std::ops::Range<usize>> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|p| (p, p + 2));
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| (p, p + 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a.0..a.1 } else { b.0..b.1 }),
        (Some(a), None) => Some(a.0..a.1),
        (None, Some(b)) => Some(b.0..b.1),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_boundaries_are_found_for_lf_and_crlf() {
        assert_eq!(find_event_end(b"data: a\n\ndata: b"), Some(7..9));
        assert_eq!(find_event_end(b"data: a\r\n\r\nrest"), Some(7..11));
        assert_eq!(find_event_end(b"data: partial"), None);
    }
}

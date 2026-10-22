// SPDX-License-Identifier: Apache-2.0
//! Pure functions for reading token usage out of LM Studio traffic and for
//! fingerprinting prompts. Nothing here does I/O, so it is unit-tested below.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Instant;

/// The inference endpoints whose traffic is counted. Anything else
/// (model listing, model load/unload, health checks) is passed through
/// untouched and not recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    /// OpenAI `/v1/chat/completions` and LM Studio `/api/v0/chat/completions`
    ChatCompletions,
    /// OpenAI `/v1/completions` and LM Studio `/api/v0/completions`
    Completions,
    /// `/v1/embeddings`, `/api/v0/embeddings`
    Embeddings,
    /// OpenAI Responses API `/v1/responses`
    Responses,
    /// Anthropic-compatible `/v1/messages`
    Messages,
    /// LM Studio native `/api/v1/chat`
    NativeChat,
}

impl EndpointKind {
    /// Endpoints that understand OpenAI's `stream_options.include_usage`.
    pub fn supports_include_usage(self) -> bool {
        matches!(
            self,
            EndpointKind::ChatCompletions | EndpointKind::Completions
        )
    }
}

pub fn classify(path: &str) -> Option<EndpointKind> {
    let p = path.split('?').next().unwrap_or("").trim_end_matches('/');
    if p.ends_with("/chat/completions") {
        Some(EndpointKind::ChatCompletions)
    } else if p.ends_with("/completions") {
        Some(EndpointKind::Completions)
    } else if p.ends_with("/embeddings") {
        Some(EndpointKind::Embeddings)
    } else if p.ends_with("/responses") {
        Some(EndpointKind::Responses)
    } else if p.ends_with("/messages") {
        Some(EndpointKind::Messages)
    } else if p == "/api/v1/chat" {
        Some(EndpointKind::NativeChat)
    } else {
        None
    }
}

/// Fields of a request body that carry prompt content. All present ones are
/// hashed together, so the fingerprint changes if any part of the prompt
/// (system prompt, history, new turn, attachments) changes.
pub const PROMPT_FIELDS: [&str; 5] = ["system", "instructions", "messages", "prompt", "input"];

/// SHA-256 (hex) of the prompt-bearing fields of a request body, serialized
/// canonically (object keys sorted, no whitespace). Returns `None` when the
/// body has none of those fields. The prompt itself is never stored.
pub fn prompt_fingerprint(body: &Value) -> Option<String> {
    let obj = body.as_object()?;
    let mut picked = serde_json::Map::new();
    for k in PROMPT_FIELDS {
        if let Some(v) = obj.get(k) {
            picked.insert(k.to_string(), v.clone());
        }
    }
    if picked.is_empty() {
        return None;
    }
    let mut canon = String::new();
    write_canonical(&Value::Object(picked), &mut canon);
    Some(hex::encode(Sha256::digest(canon.as_bytes())))
}

/// Deterministic JSON: object keys sorted by byte order, compact separators,
/// strings escaped by serde_json. Independent of serde_json's map ordering.
pub fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                write_canonical(&m[k.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
    }
}

/// Token counts reported by the server. Values are merged by taking the
/// maximum, which handles both cumulative and final-only reporting
/// (e.g. Anthropic's message_start + message_delta).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// Every input token the model read, including what the provider served
    /// from its prompt cache. Anthropic reports the cached part separately
    /// (`cache_read_input_tokens`, `cache_creation_input_tokens`) and leaves it
    /// out of `input_tokens`; it is added back here. OpenAI's `prompt_tokens`
    /// already includes its `cached_tokens`.
    pub input: Option<u64>,
    pub output: Option<u64>,
    /// Input tokens the provider served from its cache.
    pub cache_read: u64,
    /// Input tokens the provider wrote to its cache.
    pub cache_write: u64,
}

impl Usage {
    pub fn is_reported(&self) -> bool {
        self.input.is_some() || self.output.is_some()
    }
    fn merge_in(&mut self, v: u64) {
        self.input = Some(self.input.map_or(v, |x| x.max(v)));
    }
    fn merge_out(&mut self, v: u64) {
        self.output = Some(self.output.map_or(v, |x| x.max(v)));
    }
}

const INPUT_KEYS: [&str; 3] = ["prompt_tokens", "input_tokens", "prompt_tokens_count"];
const OUTPUT_KEYS: [&str; 4] = [
    "completion_tokens",
    "output_tokens",
    "total_output_tokens",
    "predicted_tokens_count",
];

/// Finds `usage` / `stats` objects anywhere in a response JSON (top level,
/// `response.usage`, `message.usage` ...) and merges their counts into `u`.
pub fn scan_usage(v: &Value, u: &mut Usage) {
    scan_usage_depth(v, u, 0)
}

fn scan_usage_depth(v: &Value, u: &mut Usage, depth: u8) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                if (k == "usage" || k == "stats") && val.is_object() {
                    // Anthropic: cached input is reported beside input_tokens.
                    let cache_read = val
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cache_write = val
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let anthropic_style = val.get("cache_read_input_tokens").is_some()
                        || val.get("cache_creation_input_tokens").is_some();
                    for ik in INPUT_KEYS {
                        if let Some(n) = val.get(ik).and_then(Value::as_u64) {
                            u.merge_in(if anthropic_style {
                                n + cache_read + cache_write
                            } else {
                                n
                            });
                        }
                    }
                    // OpenAI: prompt_tokens_details.cached_tokens is a part of prompt_tokens.
                    let openai_cached = val
                        .pointer("/prompt_tokens_details/cached_tokens")
                        .or_else(|| val.pointer("/input_tokens_details/cached_tokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    u.cache_read = u.cache_read.max(cache_read.max(openai_cached));
                    u.cache_write = u.cache_write.max(cache_write);
                    for ok in OUTPUT_KEYS {
                        if let Some(n) = val.get(ok).and_then(Value::as_u64) {
                            u.merge_out(n);
                        }
                    }
                } else if val.is_object() || val.is_array() {
                    scan_usage_depth(val, u, depth + 1);
                }
            }
        }
        Value::Array(a) => {
            for x in a {
                scan_usage_depth(x, u, depth + 1);
            }
        }
        _ => {}
    }
}

/// The model name a response reports: `model`, `response.model` or `message.model`.
pub fn response_model(v: &Value) -> Option<String> {
    if let Some(s) = v.get("model").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    for wrapper in ["response", "message"] {
        if let Some(s) = v
            .get(wrapper)
            .and_then(|w| w.get("model"))
            .and_then(Value::as_str)
        {
            return Some(s.to_string());
        }
    }
    None
}

/// Number of generated-token deltas in one streamed event. Used as a
/// fallback count when the server does not report usage (LM Studio emits
/// roughly one token per delta), and to time the first token.
pub fn content_deltas(v: &Value) -> u64 {
    let mut n = 0;
    if let Some(choices) = v.get("choices").and_then(Value::as_array) {
        for c in choices {
            let delta = c.get("delta");
            let has =
                |val: Option<&Value>| val.and_then(Value::as_str).is_some_and(|s| !s.is_empty());
            if has(delta.and_then(|d| d.get("content")))
                || has(delta.and_then(|d| d.get("reasoning_content")))
                || has(delta.and_then(|d| d.get("reasoning")))
                || has(c.get("text"))
                || delta
                    .and_then(|d| d.get("tool_calls"))
                    .is_some_and(|t| !t.is_null())
            {
                n += 1;
            }
        }
    }
    match v.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta")
        | Some("response.reasoning_text.delta")
        | Some("content_block_delta") => n += 1,
        _ => {}
    }
    n
}

/// The text a streamed event carries: (answer, reasoning).
pub fn delta_text(v: &Value, content: &mut String, reasoning: &mut String) {
    let s = |val: Option<&Value>| val.and_then(Value::as_str).map(str::to_string);
    if let Some(choices) = v.get("choices").and_then(Value::as_array) {
        for c in choices {
            let d = c.get("delta");
            if let Some(t) = s(d.and_then(|d| d.get("content"))).or_else(|| s(c.get("text"))) {
                content.push_str(&t);
            }
            if let Some(t) = s(d.and_then(|d| d.get("reasoning_content")))
                .or_else(|| s(d.and_then(|d| d.get("reasoning"))))
            {
                reasoning.push_str(&t);
            }
        }
    }
    match v.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => {
            content.push_str(&s(v.get("delta")).unwrap_or_default())
        }
        Some("response.reasoning_text.delta") => {
            reasoning.push_str(&s(v.get("delta")).unwrap_or_default())
        }
        Some("content_block_delta") => {
            let d = v.get("delta");
            content.push_str(&s(d.and_then(|d| d.get("text"))).unwrap_or_default());
            reasoning.push_str(&s(d.and_then(|d| d.get("thinking"))).unwrap_or_default());
        }
        _ => {}
    }
}

/// Splits an SSE byte stream into events and tracks usage as they pass.
/// Events are returned verbatim (bytes untouched) unless they are the
/// usage-only chunk Toktail asked for on the client's behalf.
pub struct SseTracker {
    buf: Vec<u8>,
    pub usage: Usage,
    pub model: Option<String>,
    pub deltas: u64,
    pub first_token_at: Option<Instant>,
    /// Drop the final usage-only chunk because the client never asked for it.
    strip_injected_usage: bool,
    /// When saving responses: the streamed answer and reasoning text.
    pub text: Option<(String, String)>,
    /// Most bytes kept of each of the answer and the reasoning.
    text_cap: usize,
    /// Characters of answer text seen, including any past `text_cap`.
    pub content_chars: u64,
    pub refusal: bool,
    pub tool_call: bool,
    /// The stream's last event went by (`[DONE]`, `message_stop`,
    /// `response.completed`): the answer is complete even if the server
    /// keeps the connection open.
    pub ended: bool,
}

impl SseTracker {
    pub fn new(strip_injected_usage: bool) -> Self {
        SseTracker {
            buf: Vec::new(),
            usage: Usage::default(),
            model: None,
            deltas: 0,
            first_token_at: None,
            strip_injected_usage,
            text: None,
            text_cap: 0,
            content_chars: 0,
            refusal: false,
            tool_call: false,
            ended: false,
        }
    }

    /// Also collect the streamed text (for saving responses).
    pub fn collect_text(mut self, cap: usize) -> Self {
        self.text = Some((String::new(), String::new()));
        self.text_cap = cap;
        self
    }

    /// Feed a chunk from upstream; returns the bytes to forward to the client.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::with_capacity(chunk.len());
        while let Some(end) = find_event_end(&self.buf) {
            let event: Vec<u8> = self.buf.drain(..end).collect();
            if self.observe(&event) {
                out.extend_from_slice(&event);
            }
        }
        out
    }

    /// Call at end of stream; returns any trailing bytes to forward.
    pub fn finish(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.buf);
        if rest.is_empty() {
            return rest;
        }
        if self.observe(&rest) {
            rest
        } else {
            Vec::new()
        }
    }

    /// Inspects one event. Returns false if it should not be forwarded.
    fn observe(&mut self, event: &[u8]) -> bool {
        let text = String::from_utf8_lossy(event);
        let mut data = String::new();
        for line in text.lines() {
            if let Some(d) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(d.strip_prefix(' ').unwrap_or(d));
            }
        }
        let data = data.trim();
        if data == "[DONE]" {
            self.ended = true;
            return true;
        }
        if data.is_empty() {
            return true;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return true;
        };
        self.ended |= matches!(
            v["type"].as_str(),
            Some("message_stop" | "response.completed" | "response.failed" | "response.incomplete")
        );
        self.refusal |= v
            .pointer("/delta/stop_reason")
            .is_some_and(|s| s == "refusal")
            || v.pointer("/choices/0/delta/refusal")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            || v["type"] == "response.refusal.delta"
            || v.pointer("/choices/0/finish_reason")
                .is_some_and(|s| s == "content_filter");
        self.tool_call |= v["type"] == "response.function_call_arguments.delta"
            || v.pointer("/item/type")
                .is_some_and(|s| s == "function_call")
            || v.pointer("/choices/0/delta/tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty());
        scan_usage(&v, &mut self.usage);
        if self.model.is_none() {
            self.model = response_model(&v);
        }
        let d = content_deltas(&v);
        if d > 0 && self.first_token_at.is_none() {
            self.first_token_at = Some(Instant::now());
        }
        self.deltas += d;
        if let Some((c, r)) = self.text.as_mut() {
            // Count every character, keep at most `text_cap` bytes of each.
            let (mut dc, mut dr) = (String::new(), String::new());
            delta_text(&v, &mut dc, &mut dr);
            self.content_chars += dc.chars().count() as u64;
            for (buf, add) in [(c, dc), (r, dr)] {
                let room = self.text_cap.saturating_sub(buf.len());
                if room >= add.len() {
                    buf.push_str(&add);
                } else if room > 0 {
                    let mut n = room;
                    while !add.is_char_boundary(n) {
                        n -= 1;
                    }
                    buf.push_str(&add[..n]);
                }
            }
        }
        if self.strip_injected_usage {
            let empty_choices = v
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(|c| c.is_empty());
            let has_usage = v.get("usage").is_some_and(|u| !u.is_null());
            if empty_choices && has_usage {
                return false;
            }
        }
        true
    }
}

/// Position just past the first blank-line event terminator, if any.
fn find_event_end(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < buf.len() {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some(i + 2);
        }
        if i + 3 < buf.len() && &buf[i..i + 4] == b"\r\n\r\n" {
            return Some(i + 4);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_endpoints() {
        assert_eq!(
            classify("/v1/chat/completions"),
            Some(EndpointKind::ChatCompletions)
        );
        assert_eq!(
            classify("/api/v0/chat/completions/"),
            Some(EndpointKind::ChatCompletions)
        );
        assert_eq!(
            classify("/v1/completions?x=1"),
            Some(EndpointKind::Completions)
        );
        assert_eq!(classify("/v1/embeddings"), Some(EndpointKind::Embeddings));
        assert_eq!(classify("/v1/responses"), Some(EndpointKind::Responses));
        assert_eq!(classify("/v1/messages"), Some(EndpointKind::Messages));
        assert_eq!(classify("/api/v1/chat"), Some(EndpointKind::NativeChat));
        assert_eq!(classify("/v1/models"), None);
        assert_eq!(classify("/api/v1/models/load"), None);
    }

    #[test]
    fn fingerprint_ignores_key_order_and_non_prompt_fields() {
        let a = json!({"model":"a","temperature":0.2,"messages":[{"role":"user","content":"hi"}]});
        let b = json!({"messages":[{"content":"hi","role":"user"}],"model":"b","stream":true});
        assert_eq!(prompt_fingerprint(&a), prompt_fingerprint(&b));
        let c = json!({"messages":[{"role":"user","content":"hi!"}]});
        assert_ne!(prompt_fingerprint(&a), prompt_fingerprint(&c));
        assert_eq!(prompt_fingerprint(&json!({"model":"x"})), None);
        // Known value: sha256 of {"messages":[{"content":"hi","role":"user"}]}
        assert_eq!(
            prompt_fingerprint(&a).unwrap(),
            hex::encode(Sha256::digest(
                br#"{"messages":[{"content":"hi","role":"user"}]}"#
            ))
        );
    }

    #[test]
    fn usage_shapes() {
        let mut u = Usage::default();
        scan_usage(
            &json!({"usage":{"prompt_tokens":12,"completion_tokens":34,"total_tokens":46}}),
            &mut u,
        );
        assert_eq!(
            u,
            Usage {
                input: Some(12),
                output: Some(34),
                ..Default::default()
            }
        );

        let mut u = Usage::default();
        scan_usage(
            &json!({"type":"response.completed","response":{"model":"m","usage":{"input_tokens":5,"output_tokens":9}}}),
            &mut u,
        );
        assert_eq!(
            u,
            Usage {
                input: Some(5),
                output: Some(9),
                ..Default::default()
            }
        );

        let mut u = Usage::default();
        scan_usage(
            &json!({"type":"message_start","message":{"usage":{"input_tokens":25,"output_tokens":1}}}),
            &mut u,
        );
        scan_usage(
            &json!({"type":"message_delta","usage":{"output_tokens":15}}),
            &mut u,
        );
        assert_eq!(
            u,
            Usage {
                input: Some(25),
                output: Some(15),
                ..Default::default()
            }
        );

        // Claude Code's turns are mostly cached input: it still counts.
        let mut u = Usage::default();
        scan_usage(
            &json!({"type":"message_start","message":{"usage":{"input_tokens":120,"cache_read_input_tokens":180_000,"cache_creation_input_tokens":2_000,"output_tokens":1}}}),
            &mut u,
        );
        assert_eq!(u.input, Some(182_120));
        assert_eq!((u.cache_read, u.cache_write), (180_000, 2_000));
        let mut u = Usage::default();
        scan_usage(
            &json!({"usage":{"prompt_tokens":5000,"completion_tokens":10,"prompt_tokens_details":{"cached_tokens":4096}}}),
            &mut u,
        );
        assert_eq!((u.input, u.cache_read), (Some(5000), 4096));

        let mut u = Usage::default();
        scan_usage(
            &json!({"usage":{"prompt_tokens":7,"total_tokens":7}}),
            &mut u,
        );
        assert_eq!(
            u,
            Usage {
                input: Some(7),
                output: None,
                ..Default::default()
            }
        );
    }

    #[test]
    fn sse_counts_and_strips_injected_usage() {
        let stream = concat!(
            "data: {\"model\":\"qwen\",\"choices\":[{\"delta\":{\"content\":\"Hel\"}}],\"usage\":null}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}],\"usage\":null}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );
        let bytes = stream.as_bytes();
        let mut t = SseTracker::new(true);
        let mut out = Vec::new();
        for piece in bytes.chunks(7) {
            out.extend(t.push(piece));
        }
        out.extend(t.finish());
        let out = String::from_utf8(out).unwrap();
        assert!(
            !out.contains("prompt_tokens"),
            "usage chunk should be stripped"
        );
        assert!(out.contains("[DONE]") && out.contains("Hel"));
        assert_eq!(
            t.usage,
            Usage {
                input: Some(10),
                output: Some(2),
                ..Default::default()
            }
        );
        assert_eq!(t.deltas, 2);
        assert_eq!(t.model.as_deref(), Some("qwen"));

        let mut t = SseTracker::new(false);
        let out = t.push(bytes);
        assert_eq!(
            out, bytes,
            "nothing stripped when the client asked for usage"
        );
    }
}

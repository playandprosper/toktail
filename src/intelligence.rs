// SPDX-License-Identifier: Apache-2.0
//! Transparent signals computed from bounded output, never a quality score.
use serde_json::Value;

pub fn classify(status: u16, body: &[u8]) -> (String, String, u64) {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let summary = crate::records::summarize_response(body);
    let text = summary["content"].as_str().unwrap_or("");
    // A stream summary counts every character even when only the start of
    // the answer was kept in memory.
    let chars = v["content_chars"]
        .as_u64()
        .unwrap_or_else(|| text.chars().count() as u64);
    let explicit = v["stop_reason"] == "refusal"
        || v.pointer("/choices/0/message/refusal")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
        || v.get("refusal").and_then(Value::as_bool) == Some(true)
        || v.get("output").and_then(Value::as_array).is_some_and(|a| {
            a.iter().any(|m| {
                m.get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|c| c.iter().any(|p| p["type"] == "refusal"))
            })
        })
        || v.get("choices")
            .and_then(Value::as_array)
            .is_some_and(|c| c.iter().any(|x| x["finish_reason"] == "content_filter"));
    let opening = text
        .trim_start()
        .chars()
        .take(300)
        .collect::<String>()
        .to_lowercase()
        .replace('’', "'");
    let lead = opening
        .strip_prefix("i'm sorry, but ")
        .or_else(|| opening.strip_prefix("sorry, but "))
        .unwrap_or(&opening);
    let likely = [
        "i cannot help with",
        "i can't help with",
        "i cannot assist with",
        "i can't assist with",
        "i'm unable to help with",
        "i am unable to assist with",
        "i cannot provide instructions",
        "i can't provide instructions",
        "i cannot fulfill",
        "i can't comply",
        "je ne peux pas vous aider",
        "no puedo ayudar",
        "ich kann dir dabei nicht helfen",
    ]
    .iter()
    .any(|p| lead.starts_with(p));
    if status == 499 {
        return ("cancelled".into(), "".into(), chars);
    }
    if status >= 400 {
        return ("error".into(), "".into(), chars);
    }
    if explicit || likely {
        return (
            "refused".into(),
            if explicit {
                "explicit_refusal"
            } else {
                "likely_refusal"
            }
            .into(),
            chars,
        );
    }
    let tools = v["output"]
        .as_array()
        .is_some_and(|a| a.iter().any(|x| x["type"] == "function_call"))
        || v.pointer("/choices/0/message/tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty())
        || v["tool_call"] == true;
    let signal = if tools {
        "tool_call"
    } else if text.contains("```")
        || serde_json::from_str::<Value>(text).is_ok_and(|v| v.is_object() || v.is_array())
    {
        "structured"
    } else if chars >= 1200 {
        "substantial"
    } else {
        "brief"
    };
    (
        if chars == 0 && !tools {
            "empty"
        } else {
            "complete"
        }
        .into(),
        signal.into(),
        chars,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_only_with_evidence() {
        let body = |t: &str| {
            serde_json::to_vec(&serde_json::json!({"choices":[{"message":{"content":t}}]})).unwrap()
        };
        assert_eq!(
            classify(200, &body("I'm sorry, but I can't help with that request.")).0,
            "refused"
        );
        assert_eq!(
            classify(200, &body("The server says 'I can't connect'. Restart it.")).0,
            "complete"
        );
        assert_eq!(classify(503, &body("I can't assist with that")).0, "error");
        assert_eq!(
            classify(
                200,
                br#"{"choices":[{"message":{"content":"","refusal":"Not allowed"}}]}"#
            )
            .1,
            "explicit_refusal"
        );
        assert_eq!(
            classify(200, &body("```rust\nfn main() {}\n```")).1,
            "structured"
        );
    }
}

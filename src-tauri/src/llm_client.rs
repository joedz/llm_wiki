// Lightweight LLM client for the code-wiki pipeline. Supports the
// same providers as the chat panel: Anthropic, OpenAI, and any
// OpenAI-compatible endpoint (Ollama, custom). Uses `reqwest` for
// HTTP and a minimal JSON shape — no streaming.
//
// The chat panel and the pipeline share the same LlmConfig shape
// on the TS side; we deserialize the subset we need here and route
// the request to the right provider.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    Anthropic,
    Openai,
    Custom,
    Ollama,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub provider: LlmProvider,
    pub api_key: String,
    pub model: String,
    /// OpenAI-compatible endpoint base URL (no trailing slash). For
    /// `ollama` defaults to `http://127.0.0.1:11434/v1`. For
    /// `anthropic` defaults to the official endpoint.
    #[serde(default)]
    pub base_url: Option<String>,
    pub system: String,
    pub user: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

fn default_max_tokens() -> u32 {
    4096
}
fn default_temperature() -> f32 {
    0.2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmError {
    pub kind: String,
    pub message: String,
    pub status: Option<u16>,
}

/// Strip a leading `Bearer ` if present (it shouldn't be on
/// `api_key` but defensive).
fn normalize_key(k: &str) -> &str {
    let trimmed = k.trim();
    if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("bearer ") {
        trimmed[7..].trim_start()
    } else {
        trimmed
    }
}

/// Resolve the chat-completions URL for the given provider.
fn endpoint_for(req: &LlmRequest) -> String {
    let base = req
        .base_url
        .as_deref()
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty());
    match req.provider {
        LlmProvider::Anthropic => {
            // Anthropic Messages API is /v1/messages, NOT the
            // OpenAI-compatible /v1/messages. We hardcode it.
            base.map(|b| format!("{b}/v1/messages"))
                .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string())
        }
        LlmProvider::Openai => base
            .map(|b| format!("{b}/chat/completions"))
            .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".to_string()),
        LlmProvider::Ollama => format!(
            "{}/chat/completions",
            base.unwrap_or("http://127.0.0.1:11434/v1")
        ),
        LlmProvider::Custom => {
            // Custom providers expose their own /chat/completions
            // style endpoint. The base URL is whatever the user
            // configured; we append /chat/completions if it
            // doesn't already include it.
            let b = base.unwrap_or("https://api.openai.com/v1");
            if b.ends_with("/chat/completions") {
                b.to_string()
            } else {
                format!("{b}/chat/completions")
            }
        }
    }
}

/// Make the LLM API call. Retries up to `retries` times on
/// transient network errors. Returns the response text and token
/// usage (best-effort; some providers don't report tokens).
pub async fn call_llm(req: LlmRequest, retries: u32) -> Result<LlmResponse, LlmError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| LlmError {
            kind: "build_client".to_string(),
            message: e.to_string(),
            status: None,
        })?;

    let mut last_err: Option<LlmError> = None;
    for attempt in 0..=retries {
        let result = match req.provider {
            LlmProvider::Anthropic => call_anthropic(&client, &req).await,
            LlmProvider::Openai
            | LlmProvider::Custom
            | LlmProvider::Ollama => call_openai_compat(&client, &req).await,
        };
        match result {
            Ok(r) => return Ok(r),
            Err(e) => {
                let is_transient = matches!(e.status, Some(429 | 500 | 502 | 503 | 504))
                    || e.kind == "network";
                if !is_transient || attempt == retries {
                    return Err(e);
                }
                // Exponential backoff: 1s, 2s, 4s, ...
                let backoff_ms = 1000u64 << attempt;
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| LlmError {
        kind: "unknown".to_string(),
        message: "LLM call failed without a recorded error".to_string(),
        status: None,
    }))
}

async fn call_anthropic(
    client: &reqwest::Client,
    req: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let url = endpoint_for(req);
    let body = serde_json::json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "temperature": req.temperature,
        "system": req.system,
        "messages": [
            {"role": "user", "content": req.user},
        ],
    });
    let api_key = normalize_key(&req.api_key);
    let resp = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError {
            kind: "network".to_string(),
            message: e.to_string(),
            status: None,
        })?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(LlmError {
            kind: "http_error".to_string(),
            message: text,
            status: Some(status),
        });
    }
    let parsed: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| LlmError {
            kind: "parse".to_string(),
            message: e.to_string(),
            status: Some(status),
        })?;
    // Anthropic response shape: { content: [{ type: "text", text: "..." }], usage: { input_tokens, output_tokens } }
    let content = parsed["content"]
        .as_array()
        .and_then(|arr| arr.iter().find(|c| c["type"].as_str() == Some("text")))
        .and_then(|c| c["text"].as_str())
        .unwrap_or("")
        .to_string();
    let input_tokens = parsed["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
    let output_tokens = parsed["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
    Ok(LlmResponse {
        content,
        input_tokens,
        output_tokens,
    })
}

async fn call_openai_compat(
    client: &reqwest::Client,
    req: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let url = endpoint_for(req);
    let body = serde_json::json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "temperature": req.temperature,
        "messages": [
            {"role": "system", "content": req.system},
            {"role": "user", "content": req.user},
        ],
    });
    let api_key = normalize_key(&req.api_key);
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| LlmError {
            kind: "network".to_string(),
            message: e.to_string(),
            status: None,
        })?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(LlmError {
            kind: "http_error".to_string(),
            message: text,
            status: Some(status),
        });
    }
    let parsed: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| LlmError {
            kind: "parse".to_string(),
            message: e.to_string(),
            status: Some(status),
        })?;
    // OpenAI shape: { choices: [{ message: { content: "..." } }], usage: { prompt_tokens, completion_tokens } }
    let content = parsed["choices"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|c| c["message"]["content"].as_str())
        .unwrap_or("")
        .to_string();
    let input_tokens = parsed["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
    let output_tokens = parsed["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;
    Ok(LlmResponse {
        content,
        input_tokens,
        output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_for_anthropic_default() {
        let req = LlmRequest {
            provider: LlmProvider::Anthropic,
            api_key: "k".to_string(),
            model: "claude-3-5-sonnet-latest".to_string(),
            base_url: None,
            system: "s".to_string(),
            user: "u".to_string(),
            max_tokens: 1024,
            temperature: 0.2,
        };
        assert_eq!(
            endpoint_for(&req),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn endpoint_for_openai_default() {
        let req = LlmRequest {
            provider: LlmProvider::Openai,
            api_key: "k".to_string(),
            model: "gpt-4o".to_string(),
            base_url: None,
            system: "s".to_string(),
            user: "u".to_string(),
            max_tokens: 1024,
            temperature: 0.2,
        };
        assert_eq!(
            endpoint_for(&req),
            "https://api.openai.com/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_for_ollama_default() {
        let req = LlmRequest {
            provider: LlmProvider::Ollama,
            api_key: "k".to_string(),
            model: "llama3".to_string(),
            base_url: None,
            system: "s".to_string(),
            user: "u".to_string(),
            max_tokens: 1024,
            temperature: 0.2,
        };
        assert_eq!(
            endpoint_for(&req),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_for_custom_appends_chat_completions() {
        let req = LlmRequest {
            provider: LlmProvider::Custom,
            api_key: "k".to_string(),
            model: "x".to_string(),
            base_url: Some("https://api.example.com/v1".to_string()),
            system: "s".to_string(),
            user: "u".to_string(),
            max_tokens: 1024,
            temperature: 0.2,
        };
        assert_eq!(
            endpoint_for(&req),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_for_custom_preserves_explicit_chat_completions() {
        let req = LlmRequest {
            provider: LlmProvider::Custom,
            api_key: "k".to_string(),
            model: "x".to_string(),
            base_url: Some("https://api.example.com/chat/completions".to_string()),
            system: "s".to_string(),
            user: "u".to_string(),
            max_tokens: 1024,
            temperature: 0.2,
        };
        assert_eq!(
            endpoint_for(&req),
            "https://api.example.com/chat/completions"
        );
    }

    #[test]
    fn endpoint_for_anthropic_with_base_url() {
        let req = LlmRequest {
            provider: LlmProvider::Anthropic,
            api_key: "k".to_string(),
            model: "claude-3-5-sonnet".to_string(),
            base_url: Some("https://proxy.example.com".to_string()),
            system: "s".to_string(),
            user: "u".to_string(),
            max_tokens: 1024,
            temperature: 0.2,
        };
        assert_eq!(endpoint_for(&req), "https://proxy.example.com/v1/messages");
    }

    #[test]
    fn normalize_key_strips_bearer_prefix() {
        assert_eq!(normalize_key("Bearer abc"), "abc");
        assert_eq!(normalize_key("abc"), "abc");
        assert_eq!(normalize_key("  Bearer abc  "), "abc");
    }
}

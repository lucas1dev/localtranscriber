//! Provedores por API HTTP: OpenAI (Responses), Anthropic (Messages) e Gemini.
//! Corpo, cabeçalhos e leitura da resposta copiados de `ai_engine.py`.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{label, ProviderId};
use crate::error::AppError;
use crate::summarize::Llm;

const TIMEOUT: Duration = Duration::from_secs(240);

pub(super) fn url(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

/// Envia a requisição e devolve o JSON da resposta; status fora de 2xx vira [`status_error`].
pub(super) async fn send_json(
    who: ProviderId,
    req: reqwest::RequestBuilder,
    timeout: Duration,
) -> Result<Value, AppError> {
    let resp = req.timeout(timeout).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(status_error(who, status.as_u16(), &body));
    }
    Ok(resp.json().await?)
}

/// `Internal("<rótulo> respondeu <status>: <primeiros 700 caracteres do corpo>")`.
pub(super) fn status_error(who: ProviderId, status: u16, body: &str) -> AppError {
    let body: String = body.chars().take(700).collect();
    AppError::Internal(format!("{} respondeu {status}: {body}", label(who)))
}

fn texts<'a>(items: impl IntoIterator<Item = &'a Value>) -> impl Iterator<Item = &'a str> {
    items.into_iter().map(|v| v["text"].as_str().unwrap_or(""))
}

fn array(v: &Value) -> &[Value] {
    v.as_array().map_or(&[], Vec::as_slice)
}

pub struct OpenAi {
    pub http: reqwest::Client,
    pub base: String,
    pub key: String,
    pub model: String,
}

pub struct Anthropic {
    pub http: reqwest::Client,
    pub base: String,
    pub key: String,
    pub model: String,
}

pub struct Gemini {
    pub http: reqwest::Client,
    pub base: String,
    pub key: String,
    pub model: String,
}

#[async_trait]
impl Llm for OpenAi {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AppError> {
        let payload = json!({"model": self.model, "instructions": system, "input": user});
        let req = self
            .http
            .post(url(&self.base, "/v1/responses"))
            .bearer_auth(&self.key)
            .json(&payload);
        let data = send_json(ProviderId::Openai, req, TIMEOUT).await?;
        let parts = array(&data["output"])
            .iter()
            .flat_map(|item| array(&item["content"]))
            .filter(|c| c["type"] == "output_text");
        Ok(texts(parts)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string())
    }
}

#[async_trait]
impl Llm for Anthropic {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AppError> {
        let payload = json!({
            "model": self.model,
            "max_tokens": 6000,
            "system": system,
            "messages": [{"role": "user", "content": user}],
        });
        let req = self
            .http
            .post(url(&self.base, "/v1/messages"))
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .json(&payload);
        let data = send_json(ProviderId::Anthropic, req, TIMEOUT).await?;
        let parts = array(&data["content"])
            .iter()
            .filter(|x| x["type"] == "text");
        Ok(texts(parts)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string())
    }
}

#[async_trait]
impl Llm for Gemini {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AppError> {
        let payload = json!({
            "system_instruction": {"parts": [{"text": system}]},
            "contents": [{"parts": [{"text": user}]}],
            "generationConfig": {"temperature": 0.2},
        });
        let path = format!("/v1beta/models/{}:generateContent", self.model);
        let req = self
            .http
            .post(url(&self.base, &path))
            .header("x-goog-api-key", &self.key)
            .json(&payload);
        let data = send_json(ProviderId::Gemini, req, TIMEOUT).await?;
        let parts = array(&data["candidates"])
            .iter()
            .flat_map(|c| array(&c["content"]["parts"]));
        Ok(texts(parts)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize::SYSTEM;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn openai_sends_responses_payload_and_parses_output_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(header("authorization", "Bearer K"))
            .and(body_partial_json(
                json!({"model":"gpt-5.1","instructions":SYSTEM,"input":"U"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": [
                    {"type":"reasoning","content":[]},
                    {"content":[{"type":"output_text","text":"ok"},{"type":"refusal","text":"não"}]}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let llm = OpenAi {
            http: reqwest::Client::new(),
            base: server.uri(),
            key: "K".into(),
            model: "gpt-5.1".into(),
        };
        assert_eq!(llm.complete(SYSTEM, "U").await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn anthropic_sends_headers_and_parses_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "K"))
            .and(header("anthropic-version", "2023-06-01"))
            .and(header("content-type", "application/json"))
            .and(body_partial_json(json!({
                "model":"claude-sonnet-4-5",
                "max_tokens":6000,
                "system":SYSTEM,
                "messages":[{"role":"user","content":"U"}]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content":[{"type":"text","text":" a"},{"type":"tool_use"},{"type":"text","text":"b "}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let llm = Anthropic {
            http: reqwest::Client::new(),
            base: server.uri(),
            key: "K".into(),
            model: "claude-sonnet-4-5".into(),
        };
        assert_eq!(llm.complete(SYSTEM, "U").await.unwrap(), "a\nb");
    }

    #[tokio::test]
    async fn gemini_uses_header_key_and_parses_parts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
            .and(header("x-goog-api-key", "K"))
            .and(body_partial_json(json!({
                "system_instruction":{"parts":[{"text":SYSTEM}]},
                "contents":[{"parts":[{"text":"U"}]}],
                "generationConfig":{"temperature":0.2}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates":[{"content":{"parts":[{"text":"um"},{"text":"dois"}]}}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let llm = Gemini {
            http: reqwest::Client::new(),
            base: format!("{}/", server.uri()),
            key: "K".into(),
            model: "gemini-2.5-flash".into(),
        };
        assert_eq!(llm.complete(SYSTEM, "U").await.unwrap(), "um\ndois");
    }

    #[tokio::test]
    async fn http_error_includes_status_and_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;
        let llm = OpenAi {
            http: reqwest::Client::new(),
            base: server.uri(),
            key: "K".into(),
            model: "gpt-5.1".into(),
        };
        match llm.complete(SYSTEM, "U").await {
            Err(AppError::Internal(m)) => {
                assert!(m.contains("401") && m.contains("bad key"), "{m}");
                assert_eq!(m, "OpenAI (API) respondeu 401: bad key");
            }
            other => panic!("esperava Internal, veio {other:?}"),
        }
    }

    #[tokio::test]
    async fn http_error_body_is_cut_at_700_chars() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("é".repeat(1000)))
            .mount(&server)
            .await;
        let llm = Anthropic {
            http: reqwest::Client::new(),
            base: server.uri(),
            key: "K".into(),
            model: "m".into(),
        };
        match llm.complete(SYSTEM, "U").await {
            Err(AppError::Internal(m)) => {
                assert_eq!(
                    m,
                    format!("Claude (API) respondeu 500: {}", "é".repeat(700))
                )
            }
            other => panic!("esperava Internal, veio {other:?}"),
        }
    }
}

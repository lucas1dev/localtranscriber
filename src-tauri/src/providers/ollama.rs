//! Ollama local: chat (porte de `ai_engine.py`), lista, verificação, download e remoção de modelos.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use super::http::{send_json, status_error, url};
use super::ProviderId;
use crate::error::AppError;
use crate::summarize::Llm;

const CHAT_TIMEOUT: Duration = Duration::from_secs(600);
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const ADMIN_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Ollama {
    pub http: reqwest::Client,
    pub base: String,
    pub model: String,
}

#[async_trait]
impl Llm for Ollama {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AppError> {
        let payload = json!({
            "model": self.model,
            "stream": false,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ],
            "options": {"temperature": 0.2},
        });
        let req = self.http.post(url(&self.base, "/api/chat")).json(&payload);
        let data = send_json(ProviderId::Ollama, req, CHAT_TIMEOUT).await?;
        Ok(data["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string())
    }
}

pub async fn list_models(http: &reqwest::Client, base: &str) -> Result<Vec<String>, AppError> {
    let req = http.get(url(base, "/api/tags"));
    let data = send_json(ProviderId::Ollama, req, ADMIN_TIMEOUT).await?;
    Ok(data["models"]
        .as_array()
        .map(|models| {
            models
                .iter()
                .filter_map(|m| m["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

pub async fn is_running(http: &reqwest::Client, base: &str) -> bool {
    match http
        .get(url(base, "/api/tags"))
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Uma linha NDJSON de `/api/pull`: `error` vira `User`, o resto vai para `on_progress`.
fn pull_line(
    line: &[u8],
    on_progress: &impl Fn(String, Option<u64>, Option<u64>),
) -> Result<(), AppError> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let v: Value = serde_json::from_slice(line)
        .map_err(|e| AppError::Internal(format!("Resposta inválida do Ollama: {e}")))?;
    if let Some(error) = v["error"].as_str() {
        return Err(AppError::User(error.to_string()));
    }
    let status = v["status"].as_str().unwrap_or("").to_string();
    on_progress(status, v["completed"].as_u64(), v["total"].as_u64());
    Ok(())
}

pub async fn pull(
    http: &reqwest::Client,
    base: &str,
    name: &str,
    cancel: CancellationToken,
    on_progress: impl Fn(String, Option<u64>, Option<u64>) + Send,
) -> Result<(), AppError> {
    let stream = async {
        let req = http
            .post(url(base, "/api/pull"))
            .json(&json!({"model": name, "stream": true}));
        let mut resp = req.send().await?;
        if !resp.status().is_success() {
            // Erros antes do streaming (ex.: nome inválido) vêm como `{"error": "..."}`.
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            let v: Value = serde_json::from_str(&body).unwrap_or_default();
            return Err(match v["error"].as_str() {
                Some(error) => AppError::User(error.to_string()),
                None => status_error(ProviderId::Ollama, status, &body),
            });
        }
        let mut pending: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            pending.extend_from_slice(&chunk);
            while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = pending.drain(..=pos).collect();
                pull_line(&line, &on_progress)?;
            }
        }
        pull_line(&pending, &on_progress)
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AppError::Cancelled),
        res = stream => res,
    }
}

pub async fn delete(http: &reqwest::Client, base: &str, name: &str) -> Result<(), AppError> {
    let req = http
        .delete(url(base, "/api/delete"))
        .json(&json!({"model": name}))
        .timeout(ADMIN_TIMEOUT);
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(status_error(ProviderId::Ollama, status.as_u16(), &body));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize::SYSTEM;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{body_json, body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn ollama_chat_parses_message_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(body_json(json!({
                "model":"qwen3:8b",
                "stream":false,
                "messages":[{"role":"system","content":SYSTEM},{"role":"user","content":"U"}],
                "options":{"temperature":0.2}
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"message":{"role":"assistant","content":"  resumo \n"}})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let llm = Ollama {
            http: reqwest::Client::new(),
            base: format!("{}/", server.uri()),
            model: "qwen3:8b".into(),
        };
        assert_eq!(llm.complete(SYSTEM, "U").await.unwrap(), "resumo");
    }

    #[tokio::test]
    async fn list_models_reads_tags_and_is_running() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models":[{"name":"qwen3:8b"},{"name":"llama3.2:latest"}]
            })))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        assert_eq!(
            list_models(&http, &server.uri()).await.unwrap(),
            vec!["qwen3:8b".to_string(), "llama3.2:latest".to_string()]
        );
        assert!(is_running(&http, &server.uri()).await);
    }

    #[tokio::test]
    async fn is_running_false_when_nothing_listens() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        assert!(!is_running(&reqwest::Client::new(), &base).await);
    }

    #[tokio::test]
    async fn ollama_pull_streams_progress_and_surfaces_error() {
        let server = MockServer::start().await;
        let body = concat!(
            "{\"status\":\"pulling manifest\"}\n",
            "{\"status\":\"downloading\",\"completed\":10,\"total\":100}\n",
            "{\"error\":\"not found\"}\n",
        );
        Mock::given(method("POST"))
            .and(path("/api/pull"))
            .and(body_partial_json(json!({"model":"qwen3:8b","stream":true})))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let c = calls.clone();
        let res = pull(
            &reqwest::Client::new(),
            &server.uri(),
            "qwen3:8b",
            CancellationToken::new(),
            move |s, done, total| c.lock().unwrap().push((s, done, total)),
        )
        .await;
        match res {
            Err(AppError::User(m)) => assert_eq!(m, "not found"),
            other => panic!("esperava User, veio {other:?}"),
        }
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                ("pulling manifest".to_string(), None, None),
                ("downloading".to_string(), Some(10), Some(100)),
            ]
        );
    }

    #[tokio::test]
    async fn pull_success_and_cancel() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/pull"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"status\":\"success\"}\n"))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        pull(
            &http,
            &server.uri(),
            "m",
            CancellationToken::new(),
            |_, _, _| {},
        )
        .await
        .unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();
        let res = pull(&http, &server.uri(), "m", cancel, |_, _, _| {}).await;
        assert!(matches!(res, Err(AppError::Cancelled)), "{res:?}");
    }

    #[tokio::test]
    async fn pull_http_error_with_json_is_user_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/pull"))
            .respond_with(
                ResponseTemplate::new(400).set_body_json(json!({"error":"invalid model name"})),
            )
            .mount(&server)
            .await;
        let res = pull(
            &reqwest::Client::new(),
            &server.uri(),
            "?",
            CancellationToken::new(),
            |_, _, _| {},
        )
        .await;
        assert!(
            matches!(res, Err(AppError::User(ref m)) if m == "invalid model name"),
            "{res:?}"
        );
    }

    #[tokio::test]
    async fn delete_sends_delete_with_model() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/api/delete"))
            .and(body_json(json!({"model":"qwen3:8b"})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        delete(&reqwest::Client::new(), &server.uri(), "qwen3:8b")
            .await
            .unwrap();
    }
}

use crate::error::AppError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub struct ModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub size_bytes: u64,
}

pub const CATALOG: [ModelInfo; 4] = [
    ModelInfo { id: "large-v3-turbo", label: "Large V3 Turbo — padrão", size_bytes: 1_624_555_275 },
    ModelInfo { id: "medium", label: "Medium — maior qualidade", size_bytes: 1_533_763_059 },
    ModelInfo { id: "small", label: "Small — mais leve", size_bytes: 487_601_967 },
    ModelInfo { id: "base", label: "Base — mais rápido", size_bytes: 147_951_465 },
];

pub const HF_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

pub fn file_name(id: &str) -> String {
    format!("ggml-{id}.bin")
}

pub fn find(id: &str) -> Option<&'static ModelInfo> {
    CATALOG.iter().find(|m| m.id == id)
}

#[derive(Serialize)]
pub struct ModelState {
    pub id: String,
    pub label: String,
    pub size_bytes: u64,
    pub installed: bool,
    pub is_default: bool,
}

pub fn list(models_dir: &Path, default_id: &str) -> Vec<ModelState> {
    CATALOG
        .iter()
        .map(|m| ModelState {
            id: m.id.to_string(),
            label: m.label.to_string(),
            size_bytes: m.size_bytes,
            installed: models_dir.join(file_name(m.id)).is_file(),
            is_default: m.id == default_id,
        })
        .collect()
}

pub async fn download(
    http: &reqwest::Client,
    base_url: &str,
    models_dir: &Path,
    id: &str,
    cancel: CancellationToken,
    on_progress: impl Fn(u64, Option<u64>) + Send,
) -> Result<PathBuf, AppError> {
    if find(id).is_none() {
        return Err(AppError::User(format!("Modelo desconhecido: {id}")));
    }
    tokio::fs::create_dir_all(models_dir).await?;
    let name = file_name(id);
    let final_path = models_dir.join(&name);
    let part_path = models_dir.join(format!("{name}.part"));
    let url = format!("{}/{}", base_url.trim_end_matches('/'), name);

    let result = fetch_to_part(http, &url, &part_path, &cancel, &on_progress).await;
    match result {
        Ok(()) => {
            if let Err(e) = tokio::fs::rename(&part_path, &final_path).await {
                let _ = tokio::fs::remove_file(&part_path).await;
                return Err(e.into());
            }
            Ok(final_path)
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            Err(e)
        }
    }
}

async fn fetch_to_part(
    http: &reqwest::Client,
    url: &str,
    part_path: &Path,
    cancel: &CancellationToken,
    on_progress: &impl Fn(u64, Option<u64>),
) -> Result<(), AppError> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let mut resp = tokio::select! {
        _ = cancel.cancelled() => return Err(AppError::Cancelled),
        r = http.get(url).send() => r?,
    };
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!("Download falhou: HTTP {}", resp.status().as_u16())));
    }
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(part_path).await?;
    let mut received: u64 = 0;
    let mut last_emit: Option<Instant> = None;
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            c = resp.chunk() => c?,
        };
        let Some(chunk) = chunk else { break };
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        if last_emit.map_or(true, |t| t.elapsed() >= PROGRESS_INTERVAL) {
            on_progress(received, total);
            last_emit = Some(Instant::now());
        }
    }
    file.flush().await?;
    drop(file);
    if let Some(t) = total {
        if received != t {
            return Err(AppError::Internal(format!(
                "Download incompleto: {received} de {t} bytes"
            )));
        }
    }
    on_progress(received, total);
    Ok(())
}

pub fn delete(models_dir: &Path, id: &str) -> Result<(), AppError> {
    match std::fs::remove_file(models_dir.join(file_name(id))) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub fn import_file(src: &Path, models_dir: &Path) -> Result<String, AppError> {
    let unrecognized = || {
        AppError::User(
            "Arquivo não reconhecido. Use um modelo ggml do catálogo (ex.: ggml-large-v3-turbo.bin)."
                .into(),
        )
    };
    let name = src.file_name().and_then(|n| n.to_str()).ok_or_else(unrecognized)?;
    let id = name
        .strip_prefix("ggml-")
        .and_then(|n| n.strip_suffix(".bin"))
        .and_then(find)
        .map(|m| m.id)
        .ok_or_else(unrecognized)?;
    std::fs::create_dir_all(models_dir)?;
    let dst = models_dir.join(file_name(id));
    if same_file(src, &dst) {
        // Copiar um arquivo sobre ele mesmo o zeraria.
        return Ok(id.to_string());
    }
    // Sufixo próprio: não colide com o `.part` de um download do mesmo modelo.
    let part = models_dir.join(format!("{}.import.part", file_name(id)));
    let copied = std::fs::copy(src, &part).and_then(|_| replace(&part, &dst));
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(id.to_string())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Renomeia `from` para `to`, substituindo `to` (no Windows o rename falha se `to` existe).
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Err(_) if cfg!(windows) && to.exists() => {
            std::fs::remove_file(to)?;
            std::fs::rename(from, to)
        }
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn state<'a>(v: &'a [ModelState], id: &str) -> &'a ModelState {
        v.iter().find(|m| m.id == id).unwrap()
    }

    #[test]
    fn catalog_order_and_file_name() {
        let ids: Vec<_> = CATALOG.iter().map(|m| m.id).collect();
        assert_eq!(ids, ["large-v3-turbo", "medium", "small", "base"]);
        assert_eq!(file_name("base"), "ggml-base.bin");
    }

    #[test]
    fn list_marks_installed_and_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ggml-small.bin"), b"x").unwrap();
        let v = list(dir.path(), "small");
        assert!(state(&v, "small").installed && state(&v, "small").is_default);
        assert!(!state(&v, "base").installed);
        assert!(!state(&v, "base").is_default);
    }

    #[test]
    fn part_file_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ggml-base.bin.part"), b"x").unwrap();
        assert!(!state(&list(dir.path(), "base"), "base").installed);
    }

    #[tokio::test]
    async fn download_writes_file_and_reports_progress() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ggml-base.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 10_000]))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let calls: Arc<Mutex<Vec<(u64, Option<u64>)>>> = Arc::default();
        let c2 = calls.clone();
        let p = download(
            &reqwest::Client::new(),
            &server.uri(),
            dir.path(),
            "base",
            CancellationToken::new(),
            move |r, t| c2.lock().unwrap().push((r, t)),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().len(), 10_000);
        let calls = calls.lock().unwrap();
        assert_eq!(*calls.last().unwrap(), (10_000, Some(10_000)));
        assert!(calls.iter().all(|c| c.1 == Some(10_000)));
        assert!(!dir.path().join("ggml-base.bin.part").exists());
    }

    #[tokio::test]
    async fn http_error_status_fails_and_cleans() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let r = download(&reqwest::Client::new(), &server.uri(), dir.path(), "base", CancellationToken::new(), |_, _| {}).await;
        assert!(matches!(r, Err(AppError::Internal(m)) if m == "Download falhou: HTTP 404"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn unknown_id_is_user_error() {
        let dir = tempfile::tempdir().unwrap();
        let r = download(&reqwest::Client::new(), "http://127.0.0.1:1", dir.path(), "nope", CancellationToken::new(), |_, _| {}).await;
        assert!(matches!(r, Err(AppError::User(m)) if m == "Modelo desconhecido: nope"));
    }

    #[tokio::test]
    async fn truncated_download_is_not_installed() {
        use tokio::io::AsyncReadExt;
        // Servidor manual: promete 10_000 bytes, envia 4_000 e fecha a conexão.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf).await;
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10000\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            s.write_all(&vec![1u8; 4_000]).await.unwrap();
            s.shutdown().await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let http = reqwest::Client::new();
        let r = download(&http, &format!("http://{addr}"), dir.path(), "base", CancellationToken::new(), |_, _| {}).await;
        assert!(r.is_err());
        assert!(!state(&list(dir.path(), "base"), "base").installed);
        assert!(!dir.path().join("ggml-base.bin").exists());
        assert!(!dir.path().join("ggml-base.bin.part").exists());

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 10_000]))
            .mount(&server)
            .await;
        download(&http, &server.uri(), dir.path(), "base", CancellationToken::new(), |_, _| {})
            .await
            .unwrap();
        assert!(state(&list(dir.path(), "base"), "base").installed);
    }

    #[tokio::test]
    async fn cancelled_download_cleans_part() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 10_000]))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let r = download(&reqwest::Client::new(), &server.uri(), dir.path(), "base", cancel, |_, _| {}).await;
        assert!(matches!(r, Err(AppError::Cancelled)));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancel_during_download_cleans_part() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 10_000]))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let seen = Arc::new(AtomicU64::new(0));
        let s2 = seen.clone();
        let r = download(&reqwest::Client::new(), &server.uri(), dir.path(), "base", cancel, move |b, _| {
            s2.store(b, Ordering::SeqCst);
            c2.cancel();
        })
        .await;
        assert!(matches!(r, Err(AppError::Cancelled)));
        assert!(seen.load(Ordering::SeqCst) > 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn import_rejects_unknown_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        assert!(matches!(import_file(&tmp.path().join("foo.bin"), &dir), Err(AppError::User(_))));
        assert!(matches!(import_file(&tmp.path().join("ggml-nope.bin"), &dir), Err(AppError::User(_))));
    }

    #[test]
    fn import_copies_known_model() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("ggml-small.bin");
        std::fs::write(&src, b"abc").unwrap();
        let dir = tmp.path().join("models");
        assert_eq!(import_file(&src, &dir).unwrap(), "small");
        assert_eq!(std::fs::read(dir.join("ggml-small.bin")).unwrap(), b"abc");
        assert!(src.exists());
    }

    #[test]
    fn import_of_file_already_in_models_dir_keeps_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("ggml-small.bin");
        std::fs::write(&model, vec![9u8; 4096]).unwrap();
        assert_eq!(import_file(&model, &dir).unwrap(), "small");
        // Mesmo arquivo por outro caminho (com `..`).
        let other = dir.join("..").join("models").join("ggml-small.bin");
        assert_eq!(import_file(&other, &dir).unwrap(), "small");
        assert_eq!(std::fs::metadata(&model).unwrap().len(), 4096);
        assert!(state(&list(&dir, "small"), "small").installed);
    }

    #[test]
    fn import_goes_through_part_and_replaces_old_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("models");
        std::fs::create_dir_all(&dir).unwrap();
        // Sobra de uma importação interrompida: não conta como instalado.
        std::fs::write(dir.join("ggml-small.bin.import.part"), b"meio").unwrap();
        assert!(!state(&list(&dir, "small"), "small").installed);
        std::fs::write(dir.join("ggml-small.bin"), b"antigo e maior").unwrap();
        let src = tmp.path().join("ggml-small.bin");
        std::fs::write(&src, b"novo").unwrap();
        assert_eq!(import_file(&src, &dir).unwrap(), "small");
        assert_eq!(std::fs::read(dir.join("ggml-small.bin")).unwrap(), b"novo");
        assert!(!dir.join("ggml-small.bin.import.part").exists());
    }

    #[test]
    fn delete_removes_and_tolerates_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ggml-base.bin"), b"x").unwrap();
        delete(dir.path(), "base").unwrap();
        assert!(!dir.path().join("ggml-base.bin").exists());
        delete(dir.path(), "base").unwrap();
    }
}

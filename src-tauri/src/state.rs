use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::commands::recording::RecordingHandle;
use crate::commands::transcription::CancelSlot;
use crate::credentials::{KeyringStore, SecretStore};
use crate::paths::{find_executable, AppPaths};
use crate::providers::Endpoints;
use crate::settings::Settings;
use crate::transcribe::Engine;

pub struct Sidecars {
    pub ffmpeg: PathBuf,
    pub codex: PathBuf,
}

pub struct AppState {
    pub paths: AppPaths,
    pub settings: Mutex<Settings>,
    pub engine: Arc<Engine>,
    /// Provedores de IA e Ollama: sem seguir redirecionamentos.
    pub http: reqwest::Client,
    /// Downloads de modelos Whisper: segue redirecionamentos (o Hugging Face redireciona
    /// para a CDN) e desiste se o servidor parar de mandar dados.
    pub download_http: reqwest::Client,
    pub sidecars: Sidecars,
    pub transcription_cancel: CancelSlot,
    pub recording: Mutex<Option<RecordingHandle>>,
    /// A pessoa confirmou fechar a janela mesmo com uma gravação em andamento.
    pub close_confirmed: AtomicBool,
    pub downloads: Mutex<HashMap<String, CancellationToken>>,
    pub ollama_pulls: Mutex<HashMap<String, CancellationToken>>,
    pub secrets: Arc<dyn SecretStore>,
    pub endpoints: Endpoints,
}

/// Procura `name` ao lado do executável do app; se não existir, cai para
/// os diretórios extras e o PATH. Sem achar nada, devolve o caminho esperado
/// ao lado do executável.
pub fn resolve_sidecar(
    name: &str,
    exe_dir: &Path,
    extra: &[PathBuf],
    path_var: Option<OsString>,
) -> PathBuf {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let beside = exe_dir.join(file);
    if beside.is_file() {
        return beside;
    }
    find_executable(name, extra, path_var).unwrap_or(beside)
}

/// Sem seguir redirecionamentos: `x-api-key`/`x-goog-api-key` nunca vão para outro host.
pub fn provider_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("cliente HTTP dos provedores")
}

pub const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Cliente dos downloads de modelo: erro se ficar `read_timeout` sem receber dados.
pub fn download_client(read_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .read_timeout(read_timeout)
        .build()
        .expect("cliente HTTP dos downloads")
}

impl Sidecars {
    pub fn resolve() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_default();
        let extra = [
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ];
        let path_var = std::env::var_os("PATH");
        Self {
            ffmpeg: resolve_sidecar("ffmpeg", &exe_dir, &extra, path_var.clone()),
            codex: resolve_sidecar("codex", &exe_dir, &extra, path_var),
        }
    }
}

impl AppState {
    pub fn new(paths: AppPaths) -> Self {
        let settings = crate::settings::load(&paths.settings_file);
        Self {
            paths,
            settings: Mutex::new(settings),
            engine: Arc::new(Engine::new()),
            http: provider_client(),
            download_http: download_client(DOWNLOAD_READ_TIMEOUT),
            sidecars: Sidecars::resolve(),
            transcription_cancel: CancelSlot::default(),
            recording: Mutex::new(None),
            close_confirmed: AtomicBool::new(false),
            downloads: Mutex::new(HashMap::new()),
            ollama_pulls: Mutex::new(HashMap::new()),
            secrets: Arc::new(KeyringStore),
            endpoints: Endpoints::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe_name(name: &str) -> String {
        if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        }
    }

    fn make_exe(dir: &Path, name: &str) -> PathBuf {
        let f = dir.join(exe_name(name));
        std::fs::write(&f, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        f
    }

    #[tokio::test]
    async fn provider_client_does_not_follow_redirects() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let other = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&other)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(307).insert_header("location", format!("{}/roubar", other.uri())),
            )
            .mount(&server)
            .await;
        let resp = provider_client()
            .get(server.uri())
            .header("x-api-key", "segredo")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 307);
    }

    #[test]
    fn found_next_to_exe() {
        let exe = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let beside = make_exe(exe.path(), "ffmpeg");
        make_exe(other.path(), "ffmpeg");
        let got = resolve_sidecar("ffmpeg", exe.path(), &[], Some(other.path().into()));
        assert_eq!(got, beside);
    }

    #[test]
    fn falls_back_to_path() {
        let exe = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let on_path = make_exe(other.path(), "ffmpeg");
        let got = resolve_sidecar("ffmpeg", exe.path(), &[], Some(other.path().into()));
        assert_eq!(got, on_path);
    }

    #[test]
    fn neither_gives_next_to_exe_path() {
        let exe = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        let got = resolve_sidecar("ffmpeg", exe.path(), &[], Some(empty.path().into()));
        assert_eq!(got, exe.path().join(exe_name("ffmpeg")));
    }
}

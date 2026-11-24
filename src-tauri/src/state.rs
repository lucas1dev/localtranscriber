use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

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
    pub http: reqwest::Client,
    pub sidecars: Sidecars,
    pub transcription_cancel: Mutex<Option<Arc<AtomicBool>>>,
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
            http: reqwest::Client::new(),
            sidecars: Sidecars::resolve(),
            transcription_cancel: Mutex::new(None),
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

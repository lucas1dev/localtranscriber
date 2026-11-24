use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub codex_home: PathBuf,
    pub settings_file: PathBuf,
}

impl AppPaths {
    pub fn from_data_dir(dir: PathBuf) -> std::io::Result<Self> {
        let models_dir = dir.join("models");
        let codex_home = dir.join("codex-home");
        std::fs::create_dir_all(&models_dir)?;
        std::fs::create_dir_all(&codex_home)?;
        Ok(Self {
            settings_file: dir.join("settings.json"),
            data_dir: dir,
            models_dir,
            codex_home,
        })
    }
}

pub fn find_executable(
    name: &str,
    extra_dirs: &[PathBuf],
    path_var: Option<OsString>,
) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = extra_dirs.to_vec();
    if let Some(p) = path_var {
        dirs.extend(std::env::split_paths(&p));
    }
    let candidates = executable_names(name, cfg!(windows));
    dirs.iter()
        .filter(|d| !d.as_os_str().is_empty())
        .flat_map(|d| candidates.iter().map(move |c| d.join(c)))
        .find(|f| is_executable(f))
}

/// Nomes de arquivo aceitos para `name`. No Windows só `.exe` e `.cmd`: o shim sem
/// extensão que o npm instala é um script de shell e não roda fora do Git Bash.
pub fn executable_names(name: &str, windows: bool) -> Vec<String> {
    if windows {
        vec![format!("{name}.exe"), format!("{name}.cmd")]
    } else {
        vec![name.to_string()]
    }
}

fn is_executable(f: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(f) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub fn claude_search_dirs(home: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![
            home.join(".local").join("bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]
    }
    #[cfg(target_os = "windows")]
    {
        let mut dirs = vec![home.join(".local").join("bin")];
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
        dirs
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec![home.join(".local").join("bin")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn from_data_dir_creates_subdirs() {
        let tmp = tempfile::tempdir().unwrap();
        let p = AppPaths::from_data_dir(tmp.path().to_path_buf()).unwrap();
        assert!(p.models_dir.is_dir() && p.codex_home.is_dir());
        assert_eq!(p.settings_file, tmp.path().join("settings.json"));
    }

    fn make_exe(dir: &Path, name: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let f = dir.join(name);
        fs::write(&f, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&f, fs::Permissions::from_mode(0o755)).unwrap();
        }
        f
    }

    #[test]
    fn find_executable_prefers_extra_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let extra = tmp.path().join("extra");
        let other = tmp.path().join("other");
        #[cfg(windows)]
        let file = "claude.exe";
        #[cfg(not(windows))]
        let file = "claude";
        let want = make_exe(&extra, file);
        make_exe(&other, file);
        let path_var = std::env::join_paths([&other]).unwrap();
        assert_eq!(
            find_executable("claude", &[extra], Some(path_var)),
            Some(want)
        );
    }

    #[test]
    fn find_executable_falls_back_to_path() {
        let tmp = tempfile::tempdir().unwrap();
        let other = tmp.path().join("other");
        #[cfg(windows)]
        let file = "claude.exe";
        #[cfg(not(windows))]
        let file = "claude";
        let want = make_exe(&other, file);
        let path_var = std::env::join_paths([&other]).unwrap();
        assert_eq!(find_executable("claude", &[], Some(path_var)), Some(want));
    }

    #[cfg(unix)]
    #[test]
    fn find_executable_ignores_non_executable_files() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("claude");
        fs::write(&f, "x").unwrap();
        assert!(find_executable("claude", &[tmp.path().to_path_buf()], Some("".into())).is_none());
    }

    #[test]
    fn executable_names_on_windows_only_exe_and_cmd() {
        assert_eq!(executable_names("claude", true), ["claude.exe", "claude.cmd"]);
        assert_eq!(executable_names("claude", false), ["claude"]);
    }

    #[cfg(windows)]
    #[test]
    fn find_executable_skips_extensionless_npm_shim() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("claude"), "#!/bin/sh\n").unwrap();
        let want = make_exe(tmp.path(), "claude.cmd");
        assert_eq!(
            find_executable("claude", &[tmp.path().to_path_buf()], Some("".into())),
            Some(want)
        );
    }

    #[test]
    fn find_executable_none_when_absent() {
        assert!(find_executable("nao-existe", &[], Some("".into())).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn claude_search_dirs_macos() {
        let d = claude_search_dirs(Path::new("/Users/x"));
        assert_eq!(
            d,
            vec![
                PathBuf::from("/Users/x/.local/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        );
    }
}

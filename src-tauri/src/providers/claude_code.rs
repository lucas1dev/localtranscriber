//! Claude por assinatura via Claude Code instalado e logado pela própria pessoa.
//!
//! Flags conferidas em `claude --help` (Claude Code 2.1.331):
//! - `claude auth status` — JSON (padrão) com `loggedIn`, `authMethod`, `email`.
//! - `claude -p --output-format text --no-session-persistence --system-prompt <SYSTEM>
//!   --tools "" --strict-mcp-config --safe-mode [--model <modelo>]`, prompt pelo stdin.
//!   `--tools ""` desliga todas as ferramentas embutidas; `--strict-mcp-config` sem
//!   `--mcp-config` não carrega nenhum servidor MCP; `--safe-mode` desliga CLAUDE.md,
//!   hooks, plugins e MCP da pessoa sem afetar o login. Num `claude` sem `--safe-mode`
//!   (detectado uma vez pelo `--help`), usa `--setting-sources project`.
//! - Roda num diretório temporário vazio, para não herdar o CLAUDE.md nem as
//!   configurações de projeto do diretório do app. Não usa `--bare` (desliga o OAuth).
//!
//! Nunca roda `claude login` nem lê arquivos de credencial.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;

use super::cli;
use super::codex::AuthStatus;
use crate::error::AppError;
use crate::paths::{claude_search_dirs, find_executable};
use crate::summarize::Llm;

const LOGIN_REQUIRED: &str =
    "Entre na sua conta no Claude Code (rode `claude` no Terminal) e tente de novo.";
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
const EXEC_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const HELP_TIMEOUT: Duration = Duration::from_secs(15);

/// Pastas que entram na frente do PATH herdado: a do próprio `claude` e as do Homebrew.
/// Aberto pelo Finder, o app herda um PATH mínimo, e os shims `#!/usr/bin/env node` do
/// npm não achariam o `node`.
pub fn child_path(bin: &Path, inherited: Option<&OsStr>) -> OsString {
    let mut dirs: Vec<PathBuf> = bin.parent().map(Path::to_path_buf).into_iter().collect();
    dirs.push("/opt/homebrew/bin".into());
    dirs.push("/usr/local/bin".into());
    if let Some(p) = inherited {
        dirs.extend(std::env::split_paths(p));
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| inherited.unwrap_or_default().to_owned())
}

/// Variáveis de ambiente do processo do Claude Code.
fn child_env(bin: &Path) -> Vec<(OsString, OsString)> {
    if cfg!(windows) {
        return Vec::new();
    }
    let path = child_path(bin, std::env::var_os("PATH").as_deref());
    vec![("PATH".into(), path)]
}

/// Argumentos de `claude -p`. `safe_mode`: o `claude` aceita `--safe-mode`.
pub fn exec_args(system: &str, model: Option<&str>, safe_mode: bool) -> Vec<OsString> {
    let isolation: &[&str] = if safe_mode {
        &["--safe-mode"]
    } else {
        &["--setting-sources", "project"]
    };
    let mut args: Vec<OsString> = [
        "-p",
        "--output-format",
        "text",
        "--no-session-persistence",
        "--system-prompt",
        system,
        // `--tools` aceita vários valores: o próximo argumento precisa ser uma opção.
        "--tools",
        "",
        "--strict-mcp-config",
    ]
    .iter()
    .chain(isolation)
    .map(OsString::from)
    .collect();
    if let Some(model) = model {
        args.push("--model".into());
        args.push(model.into());
    }
    args
}

/// Se o `claude` em `bin` aceita `--safe-mode`, lido do `--help` na primeira vez e
/// guardado por caminho do executável.
pub async fn supports_safe_mode(bin: &Path) -> bool {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    if let Some(&known) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(bin) {
        return known;
    }
    let args = [OsString::from("--help")];
    let env = child_env(bin);
    let supported = match cli::run("Claude Code", bin, &args, &env, None, HELP_TIMEOUT).await {
        Ok(out) => out.code == 0 && out.stdout.contains("--safe-mode"),
        Err(_) => false,
    };
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(bin.to_path_buf(), supported);
    supported
}

#[derive(Debug, Clone)]
pub struct ClaudeCode {
    pub bin: PathBuf,
}

pub fn locate(home: &Path, path_var: Option<OsString>) -> Option<ClaudeCode> {
    locate_in(&claude_search_dirs(home), path_var)
}

/// Procura o `claude` em `dirs` e depois no `path_var`.
pub fn locate_in(dirs: &[PathBuf], path_var: Option<OsString>) -> Option<ClaudeCode> {
    find_executable("claude", dirs, path_var).map(|bin| ClaudeCode { bin })
}

impl ClaudeCode {
    pub async fn status(&self) -> AuthStatus {
        if !self.bin.is_file() {
            return AuthStatus::Missing;
        }
        let args = [OsString::from("auth"), OsString::from("status")];
        let env = child_env(&self.bin);
        let out = match cli::run("Claude Code", &self.bin, &args, &env, None, STATUS_TIMEOUT).await {
            Ok(out) => out,
            Err(e) => return AuthStatus::Error(e.to_string()),
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(out.stdout.trim()) else {
            return AuthStatus::LoggedOut;
        };
        if json["loggedIn"] != serde_json::Value::Bool(true) {
            return AuthStatus::LoggedOut;
        }
        let detail = ["email", "authMethod"]
            .iter()
            .find_map(|k| json[*k].as_str().filter(|s| !s.is_empty()))
            .unwrap_or("conectado");
        AuthStatus::LoggedIn(detail.to_string())
    }

    pub fn llm(&self, model: Option<String>) -> ClaudeCodeLlm {
        ClaudeCodeLlm {
            bin: self.bin.clone(),
            model,
        }
    }
}

pub struct ClaudeCodeLlm {
    pub bin: PathBuf,
    pub model: Option<String>,
}

#[async_trait]
impl Llm for ClaudeCodeLlm {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AppError> {
        let safe_mode = supports_safe_mode(&self.bin).await;
        let args = exec_args(system, self.model.as_deref(), safe_mode);
        // Diretório vazio, apagado quando `workdir` sai de escopo (depois do processo terminar).
        let workdir = tempfile::tempdir()?;
        let out = cli::run_in(
            "Claude Code",
            &self.bin,
            &args,
            &child_env(&self.bin),
            Some(workdir.path()),
            Some(user),
            EXEC_TIMEOUT,
        )
        .await?;
        if out.code != 0 {
            return Err(cli::failure(&out, "Claude Code", LOGIN_REQUIRED, user));
        }
        let answer = out.stdout.trim();
        if answer.is_empty() {
            return Err(AppError::Internal(
                "Claude Code não devolveu resposta.".into(),
            ));
        }
        Ok(answer.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_exe(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let f = dir.join(name);
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        f
    }

    #[test]
    fn claude_locate_finds_in_local_bin() {
        let tmp = tempfile::tempdir().unwrap();
        let local_bin = tmp.path().join(".local").join("bin");
        let want = make_exe(&local_bin, "claude");
        let found = locate_in(&[local_bin], Some("".into())).expect("deveria achar o claude");
        assert_eq!(found.bin, want);
    }

    #[test]
    fn claude_locate_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let local_bin = tmp.path().join(".local").join("bin");
        assert!(locate_in(&[local_bin], Some("".into())).is_none());
    }

    fn strings(args: &[OsString]) -> Vec<&str> {
        args.iter().map(|a| a.to_str().unwrap()).collect()
    }

    #[test]
    fn exec_args_with_safe_mode() {
        assert_eq!(
            strings(&exec_args("SISTEMA", None, true)),
            [
                "-p", "--output-format", "text", "--no-session-persistence",
                "--system-prompt", "SISTEMA", "--tools", "", "--strict-mcp-config",
                "--safe-mode",
            ]
        );
        assert_eq!(
            strings(&exec_args("S", Some("sonnet"), true))[9..],
            ["--safe-mode", "--model", "sonnet"]
        );
    }

    #[test]
    fn exec_args_without_safe_mode_limits_setting_sources() {
        let args = exec_args("S", Some("opus"), false);
        let args = strings(&args);
        assert_eq!(
            args[6..],
            ["--tools", "", "--strict-mcp-config", "--setting-sources", "project", "--model", "opus"]
        );
        assert!(!args.contains(&"--safe-mode") && !args.contains(&"--bare"));
    }

    #[test]
    fn child_path_puts_bin_dir_and_homebrew_first() {
        let bin = Path::new("/Users/x/.npm-global/bin/claude");
        let want = std::env::join_paths([
            "/Users/x/.npm-global/bin",
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
        ])
        .unwrap();
        let inherited = std::env::join_paths(["/usr/bin", "/bin"]).unwrap();
        assert_eq!(child_path(bin, Some(&inherited)), want);
        let want = std::env::join_paths(["/Users/x/.npm-global/bin", "/opt/homebrew/bin", "/usr/local/bin"]).unwrap();
        assert_eq!(child_path(bin, None), want);
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use crate::providers::cli::tests::script;

        #[tokio::test]
        async fn status_spawn_failure_is_not_logged_out() {
            let tmp = tempfile::tempdir().unwrap();
            // Shim de npm cujo interpretador não existe.
            let bin = tmp.path().join("claude");
            std::fs::write(&bin, "#!/nao/existe/node\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            match (ClaudeCode { bin }).status().await {
                AuthStatus::Error(m) => {
                    assert!(m.starts_with("Não foi possível executar o Claude Code: "), "{m}")
                }
                other => panic!("esperava Error, veio {other:?}"),
            }
        }

        #[tokio::test]
        async fn child_gets_bin_dir_on_path() {
            let tmp = tempfile::tempdir().unwrap();
            let log = tmp.path().join("path.txt");
            let bin = script(
                tmp.path(),
                "claude",
                &format!(r#"echo "$PATH" > "{}"; echo '{{"loggedIn": true}}'"#, log.display()),
            );
            ClaudeCode { bin }.status().await;
            let path = std::fs::read_to_string(&log).unwrap();
            let want = format!("{}:/opt/homebrew/bin:/usr/local/bin:", tmp.path().display());
            assert!(path.starts_with(&want), "{path}");
        }

        #[tokio::test]
        async fn safe_mode_detected_once_from_help() {
            let tmp = tempfile::tempdir().unwrap();
            let count = tmp.path().join("count");
            let with = script(
                tmp.path(),
                "com",
                &format!(
                    r#"echo x >> "{}"; echo '  --safe-mode   Start with all customizations disabled'"#,
                    count.display()
                ),
            );
            assert!(supports_safe_mode(&with).await);
            assert!(supports_safe_mode(&with).await);
            assert_eq!(std::fs::read_to_string(&count).unwrap(), "x\n");
            let without = script(tmp.path(), "sem", "echo '  --setting-sources <sources>'");
            assert!(!supports_safe_mode(&without).await);
            let broken = script(tmp.path(), "quebrado", "exit 1");
            assert!(!supports_safe_mode(&broken).await);
        }

        #[tokio::test]
        async fn complete_runs_in_empty_temp_dir() {
            let tmp = tempfile::tempdir().unwrap();
            let log = tmp.path().join("log.txt");
            let bin = script(
                tmp.path(),
                "claude",
                &format!(
                    r#"[ "$1" = "--help" ] && exit 0; pwd -P > "{log}"; ls -A | wc -l | tr -d ' ' >> "{log}"; cat"#,
                    log = log.display()
                ),
            );
            ClaudeCodeLlm { bin, model: None }.complete("S", "U").await.unwrap();
            let log = std::fs::read_to_string(&log).unwrap();
            let (dir, entries) = log.trim().split_once('\n').unwrap();
            let here = std::env::current_dir().unwrap().canonicalize().unwrap();
            assert_ne!(Path::new(dir), here.as_path());
            assert_eq!(entries, "0", "o diretório de trabalho deveria estar vazio");
            assert!(!Path::new(dir).exists(), "o diretório temporário deveria ser apagado");
        }

        #[tokio::test]
        async fn status_reads_logged_in_json() {
            let tmp = tempfile::tempdir().unwrap();
            let bin = script(
                tmp.path(),
                "a",
                r#"echo '{"loggedIn": true, "authMethod": "claude.ai", "email": "p@x.com"}'"#,
            );
            assert_eq!(
                ClaudeCode { bin }.status().await,
                AuthStatus::LoggedIn("p@x.com".into())
            );
            let bin = script(
                tmp.path(),
                "b",
                r#"echo '{"loggedIn": true, "authMethod": "claude.ai"}'"#,
            );
            assert_eq!(
                ClaudeCode { bin }.status().await,
                AuthStatus::LoggedIn("claude.ai".into())
            );
            let bin = script(tmp.path(), "c", r#"echo '{"loggedIn": true}'"#);
            assert_eq!(
                ClaudeCode { bin }.status().await,
                AuthStatus::LoggedIn("conectado".into())
            );
            let bin = script(tmp.path(), "d", r#"echo '{"loggedIn": false}'; exit 1"#);
            assert_eq!(ClaudeCode { bin }.status().await, AuthStatus::LoggedOut);
            let bin = script(tmp.path(), "e", "echo 'isto não é json'");
            assert_eq!(ClaudeCode { bin }.status().await, AuthStatus::LoggedOut);
            let bin = tmp.path().join("nao-existe");
            assert_eq!(ClaudeCode { bin }.status().await, AuthStatus::Missing);
        }

        #[tokio::test]
        async fn complete_sends_flags_and_prompt_by_stdin() {
            let tmp = tempfile::tempdir().unwrap();
            let args_file = tmp.path().join("args.txt");
            let bin = script(
                tmp.path(),
                "claude",
                &format!(
                    r#"[ "$1" = "--help" ] && echo '  --safe-mode' && exit 0; printf '%s\n' "$@" > "{}"; printf '  resposta: '; cat"#,
                    args_file.display()
                ),
            );
            let llm = ClaudeCodeLlm {
                bin: bin.clone(),
                model: None,
            };
            let user = format!("MARCADOR{}", "y".repeat(50_000));
            let got = llm.complete("SISTEMA", &user).await.unwrap();
            assert_eq!(got, format!("resposta: {user}"));
            let args = std::fs::read_to_string(&args_file).unwrap();
            assert_eq!(
                args,
                "-p\n--output-format\ntext\n--no-session-persistence\n--system-prompt\nSISTEMA\n\
                 --tools\n\n--strict-mcp-config\n--safe-mode\n"
            );

            let llm = ClaudeCodeLlm {
                bin,
                model: Some("sonnet".into()),
            };
            llm.complete("S", "U").await.unwrap();
            let args = std::fs::read_to_string(&args_file).unwrap();
            assert!(
                args.ends_with("--strict-mcp-config\n--safe-mode\n--model\nsonnet\n"),
                "{args}"
            );
        }

        #[tokio::test]
        async fn complete_maps_login_and_other_failures() {
            let tmp = tempfile::tempdir().unwrap();
            let bin = script(
                tmp.path(),
                "a",
                "cat >/dev/null; echo 'Invalid API key · Please run /login'; exit 1",
            );
            match (ClaudeCodeLlm { bin, model: None })
                .complete("S", "U")
                .await
            {
                Err(AppError::User(m)) => assert!(m.contains("Claude Code"), "{m}"),
                other => panic!("esperava User, veio {other:?}"),
            }
            let bin = script(
                tmp.path(),
                "b",
                "cat >/dev/null; echo 'quebrou' >&2; exit 2",
            );
            match (ClaudeCodeLlm { bin, model: None })
                .complete("S", "U")
                .await
            {
                Err(AppError::Internal(m)) => assert_eq!(m, "Claude Code falhou: quebrou"),
                other => panic!("esperava Internal, veio {other:?}"),
            }
        }

        #[tokio::test]
        async fn failure_ignores_login_words_in_echoed_prompt() {
            let tmp = tempfile::tempdir().unwrap();
            let bin = script(
                tmp.path(),
                "eco",
                "p=$(cat); printf 'user\\n%s\\n' \"$p\" >&2; echo 'falha interna' >&2; exit 2",
            );
            let user = "Pedi para faça login de novo, deu erro 401 Unauthorized.";
            match (ClaudeCodeLlm { bin, model: None })
                .complete("S", user)
                .await
            {
                Err(AppError::Internal(m)) => assert_eq!(m, "Claude Code falhou: falha interna"),
                other => panic!("esperava Internal, veio {other:?}"),
            }
        }
    }

    #[tokio::test]
    #[ignore = "manual: usa o Claude Code real; LT_REAL_CLAUDE=1 cargo test claude_real -- --ignored"]
    async fn claude_real() {
        if std::env::var_os("LT_REAL_CLAUDE").is_none() {
            eprintln!("LT_REAL_CLAUDE não definida; teste manual ignorado");
            return;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        let claude = locate(&home, std::env::var_os("PATH")).expect("Claude Code não encontrado");
        let out = claude
            .llm(None)
            .complete(
                crate::summarize::SYSTEM,
                "Resuma em uma frase: Olá, isto é um teste.",
            )
            .await
            .unwrap();
        eprintln!("resposta: {out}");
        assert!(!out.trim().is_empty());
    }
}

//! Claude por assinatura via Claude Code instalado e logado pela própria pessoa.
//!
//! Flags conferidas em `claude --help` (Claude Code 2.1.331):
//! - `claude auth status` — JSON (padrão) com `loggedIn`, `authMethod`, `email`.
//! - `claude -p --output-format text --no-session-persistence --system-prompt <SYSTEM>
//!   --tools "" --strict-mcp-config [--model <modelo>]`, prompt pelo stdin.
//!   `--tools ""` desliga todas as ferramentas embutidas; `--strict-mcp-config` sem
//!   `--mcp-config` não carrega nenhum servidor MCP.
//!
//! Nunca roda `claude login` nem lê arquivos de credencial.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
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
        let Ok(out) = cli::run("Claude Code", &self.bin, &args, &[], None, STATUS_TIMEOUT).await
        else {
            return AuthStatus::LoggedOut;
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
        .into_iter()
        .map(OsString::from)
        .collect();
        if let Some(model) = &self.model {
            args.push("--model".into());
            args.push(model.into());
        }
        let out = cli::run(
            "Claude Code",
            &self.bin,
            &args,
            &[],
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

    #[cfg(unix)]
    mod unix {
        use super::*;
        use crate::providers::cli::tests::script;

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
                    r#"printf '%s\n' "$@" > "{}"; printf '  resposta: '; cat"#,
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
                 --tools\n\n--strict-mcp-config\n"
            );

            let llm = ClaudeCodeLlm {
                bin,
                model: Some("sonnet".into()),
            };
            llm.complete("S", "U").await.unwrap();
            let args = std::fs::read_to_string(&args_file).unwrap();
            assert!(
                args.ends_with("--strict-mcp-config\n--model\nsonnet\n"),
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

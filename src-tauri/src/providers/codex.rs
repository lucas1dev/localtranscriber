//! ChatGPT por assinatura via Codex CLI embutido.
//!
//! Flags conferidas em `codex-cli 0.172.0` (release `rust-v0.172.0`, aarch64-apple-darwin):
//! - `codex login status` — sai 0 quando logado (stdout = descrição do login); sem login, sai 1
//!   com "Not logged in".
//! - `codex login` — abre o navegador; `codex logout` — sai 0 mesmo sem login.
//! - `codex exec --sandbox read-only --skip-git-repo-check --ephemeral --color never
//!   -C <pasta vazia> -o <arquivo> [-m <modelo>] -`
//!   (`-` = instruções lidas do stdin; `-o` = `--output-last-message`, grava a resposta final).
//!   Sem login, `exec` sai 1 com "401 Unauthorized" no stderr.
//!
//! Toda chamada recebe `CODEX_HOME=<home>`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use super::cli;
use crate::error::AppError;
use crate::summarize::Llm;

const LOGIN_REQUIRED: &str = "Entre com sua conta ChatGPT em Configurações → Contas de IA.";
const STATUS_TIMEOUT: Duration = Duration::from_secs(30);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const EXEC_TIMEOUT: Duration = Duration::from_secs(15 * 60);

fn codex_env(home: &Path) -> [(OsString, OsString); 1] {
    [("CODEX_HOME".into(), home.as_os_str().to_owned())]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthStatus {
    LoggedIn(String),
    LoggedOut,
    Missing,
    /// O programa existe mas não pôde ser executado (ou não respondeu); a mensagem é para a pessoa.
    Error(String),
}

impl Serialize for AuthStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (state, detail) = match self {
            AuthStatus::LoggedIn(d) => ("logged_in", Some(d.as_str())),
            AuthStatus::LoggedOut => ("logged_out", None),
            AuthStatus::Missing => ("missing", None),
            AuthStatus::Error(m) => ("error", Some(m.as_str())),
        };
        let mut st = serializer.serialize_struct("AuthStatus", 2)?;
        st.serialize_field("state", state)?;
        st.serialize_field("detail", &detail)?;
        st.end()
    }
}

/// stdout sem espaços nas pontas; se vazio, a primeira linha do stderr; senão "conectado".
fn login_detail(out: &cli::CliOutput) -> String {
    let stdout = out.stdout.trim();
    let detail = if stdout.is_empty() {
        out.stderr.trim().lines().next().unwrap_or("").trim()
    } else {
        stdout
    };
    if detail.is_empty() {
        "conectado"
    } else {
        detail
    }
    .to_string()
}

/// Linhas `ERROR…`/`error:` do stderr que vêm depois do prompt ecoado: o Codex 0.172.0
/// repete no stderr o prompt inteiro (SYSTEM + transcrição) antes dos logs.
fn error_lines(out: &cli::CliOutput, prompt: &str) -> String {
    cli::after_echo(&out.stderr, prompt)
        .lines()
        .map(str::trim)
        .filter(|l| l.to_lowercase().starts_with("error"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone)]
pub struct Codex {
    pub bin: PathBuf,
    pub home: PathBuf,
}

impl Codex {
    async fn run(&self, args: &[&str], timeout: Duration) -> Result<cli::CliOutput, AppError> {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        cli::run(
            "Codex",
            &self.bin,
            &args,
            &codex_env(&self.home),
            None,
            timeout,
        )
        .await
    }

    pub async fn status(&self) -> AuthStatus {
        if !self.bin.is_file() {
            return AuthStatus::Missing;
        }
        match self.run(&["login", "status"], STATUS_TIMEOUT).await {
            Ok(out) if out.code == 0 => AuthStatus::LoggedIn(login_detail(&out)),
            _ => AuthStatus::LoggedOut,
        }
    }

    /// Abre o navegador para a pessoa entrar com a conta ChatGPT.
    pub async fn login(&self) -> Result<AuthStatus, AppError> {
        let out = self.run(&["login"], LOGIN_TIMEOUT).await?;
        if out.code != 0 {
            return Err(AppError::Internal(format!(
                "Codex falhou: {}",
                cli::tail(&out.stderr, 600)
            )));
        }
        Ok(self.status().await)
    }

    pub async fn logout(&self) -> Result<(), AppError> {
        let out = self.run(&["logout"], STATUS_TIMEOUT).await?;
        if out.code != 0 {
            return Err(AppError::Internal(format!(
                "Codex falhou: {}",
                cli::tail(&out.stderr, 600)
            )));
        }
        Ok(())
    }

    pub fn llm(&self, model: Option<String>) -> CodexLlm {
        CodexLlm {
            bin: self.bin.clone(),
            home: self.home.clone(),
            model,
        }
    }
}

pub struct CodexLlm {
    pub bin: PathBuf,
    pub home: PathBuf,
    pub model: Option<String>,
}

impl CodexLlm {
    /// Pede login se as linhas de erro (sem o prompt ecoado) falarem disso ou se
    /// `codex login status` disser que não há login; senão `Internal` com essas linhas.
    async fn failure(&self, out: &cli::CliOutput, prompt: &str) -> AppError {
        let errors = error_lines(out, prompt);
        let codex = Codex {
            bin: self.bin.clone(),
            home: self.home.clone(),
        };
        if cli::mentions_login(&errors) || !matches!(codex.status().await, AuthStatus::LoggedIn(_))
        {
            return AppError::User(LOGIN_REQUIRED.into());
        }
        if errors.is_empty() {
            return AppError::Internal(format!("Codex falhou (código {}).", out.code));
        }
        AppError::Internal(format!("Codex falhou: {}", cli::tail(&errors, 600)))
    }
}

#[async_trait]
impl Llm for CodexLlm {
    async fn complete(&self, system: &str, user: &str) -> Result<String, AppError> {
        let tmp = tempfile::tempdir()?;
        let work = tmp.path().join("work");
        std::fs::create_dir(&work)?;
        let last = tmp.path().join("last-message.txt");
        let mut args: Vec<OsString> = ["exec", "--sandbox", "read-only", "--skip-git-repo-check"]
            .into_iter()
            .chain(["--ephemeral", "--color", "never", "-C"])
            .map(OsString::from)
            .collect();
        args.push(work.into_os_string());
        args.push("-o".into());
        args.push(last.clone().into_os_string());
        if let Some(model) = &self.model {
            args.push("-m".into());
            args.push(model.into());
        }
        args.push("-".into());
        let prompt = format!("{system}\n\n{user}");
        let out = cli::run(
            "Codex",
            &self.bin,
            &args,
            &codex_env(&self.home),
            Some(&prompt),
            EXEC_TIMEOUT,
        )
        .await?;
        if out.code != 0 {
            return Err(self.failure(&out, &prompt).await);
        }
        let answer = std::fs::read_to_string(&last).unwrap_or_default();
        let answer = answer.trim();
        if answer.is_empty() {
            return Err(AppError::Internal("Codex não devolveu resposta.".into()));
        }
        Ok(answer.to_string())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::providers::cli::tests::script;

    /// Script que imita `codex exec`: grava os argumentos em `args.txt` e escreve no
    /// arquivo de `-o` o resultado de `body` (que lê o stdin).
    fn fake_exec(dir: &std::path::Path, body: &str) -> PathBuf {
        let args_file = dir.join("args.txt");
        script(
            dir,
            "codex",
            &format!(
                r#"printf '%s\n' "$@" > "{args}"
out=""
while [ $# -gt 0 ]; do
  if [ "$1" = "-o" ]; then out="$2"; fi
  shift
done
{body} > "$out""#,
                args = args_file.display()
            ),
        )
    }

    #[tokio::test]
    async fn long_prompt_goes_through_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_exec(tmp.path(), "wc -c");
        let llm = CodexLlm {
            bin,
            home: tmp.path().join("home"),
            model: None,
        };
        let user = format!("MARCADOR{}", "x".repeat(120_000));
        let got = llm.complete("SISTEMA", &user).await.unwrap();
        let n: usize = got.trim().parse().unwrap();
        assert!(n >= 120_000 + "SISTEMA\n\n".len(), "{n}");
        let args = std::fs::read_to_string(tmp.path().join("args.txt")).unwrap();
        assert!(
            !args.contains("MARCADOR") && !args.contains("xxxxxxxx"),
            "{args}"
        );
        assert!(!args.contains("SISTEMA"));
    }

    #[tokio::test]
    async fn exec_uses_readonly_flags_and_model_only_when_some() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_exec(tmp.path(), "cat");
        let llm = CodexLlm {
            bin: bin.clone(),
            home: tmp.path().join("home"),
            model: None,
        };
        assert_eq!(llm.complete("S", "U").await.unwrap(), "S\n\nU");
        let args = std::fs::read_to_string(tmp.path().join("args.txt")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(args[0], "exec");
        assert!(args.windows(2).any(|w| w == ["--sandbox", "read-only"]));
        assert!(args.contains(&"--skip-git-repo-check"));
        assert!(!args.contains(&"-m"));
        assert_eq!(args.last(), Some(&"-"));
        let cd = args[args.iter().position(|a| *a == "-C").unwrap() + 1];
        assert!(
            !std::path::Path::new(cd).exists(),
            "pasta temporária deve ser apagada"
        );

        let llm = CodexLlm {
            bin,
            home: tmp.path().join("home"),
            model: Some("gpt-5.1".into()),
        };
        llm.complete("S", "U").await.unwrap();
        let args = std::fs::read_to_string(tmp.path().join("args.txt")).unwrap();
        assert!(args.contains("-m\ngpt-5.1\n"));
    }

    #[tokio::test]
    async fn codex_receives_codex_home() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(tmp.path(), "codex", r#"echo "$CODEX_HOME""#);
        let home = tmp.path().join("codex-home");
        let codex = Codex {
            bin,
            home: home.clone(),
        };
        assert_eq!(
            codex.status().await,
            AuthStatus::LoggedIn(home.display().to_string())
        );
    }

    #[tokio::test]
    async fn codex_status_maps_exit_codes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let ok = script(tmp.path(), "ok", "echo 'Logged in using ChatGPT'; exit 0");
        let on_stderr = script(
            tmp.path(),
            "err",
            "echo; echo ' Logged in using an API key ' >&2",
        );
        let silent = script(tmp.path(), "mudo", "exit 0");
        for (bin, want) in [
            (on_stderr, "Logged in using an API key"),
            (silent, "conectado"),
        ] {
            let st = Codex {
                bin,
                home: home.clone(),
            }
            .status()
            .await;
            assert_eq!(st, AuthStatus::LoggedIn(want.into()));
        }
        let st = Codex {
            bin: ok,
            home: home.clone(),
        }
        .status()
        .await;
        assert_eq!(st, AuthStatus::LoggedIn("Logged in using ChatGPT".into()));
        let out = script(tmp.path(), "out", "echo 'Not logged in'; exit 1");
        assert_eq!(
            Codex {
                bin: out,
                home: home.clone()
            }
            .status()
            .await,
            AuthStatus::LoggedOut
        );
        let missing = tmp.path().join("nao-existe");
        assert_eq!(
            Codex { bin: missing, home }.status().await,
            AuthStatus::Missing
        );
    }

    /// Codex falso para falhas: `login status` responde conforme `logged_in`; `exec` ecoa o
    /// prompt no stderr como o 0.172.0 real e depois roda `then`.
    fn failing_codex(dir: &std::path::Path, logged_in: bool, then: &str) -> CodexLlm {
        let status = if logged_in {
            "echo 'Logged in using ChatGPT'; exit 0"
        } else {
            "echo 'Not logged in' >&2; exit 1"
        };
        let bin = script(
            dir,
            "codex",
            &format!(
                r#"if [ "$1" = "login" ]; then {status}; fi
p=$(cat)
printf 'OpenAI Codex v0.172.0\n--------\nuser\n%s\n\n' "$p" >&2
{then}"#
            ),
        );
        CodexLlm {
            bin,
            home: dir.join("home"),
            model: None,
        }
    }

    /// Transcrição que fala de login e 401 e tem até uma linha começando por "ERROR".
    const TRICKY: &str =
        "Pedi para faça login de novo, deu erro 401 Unauthorized.\nERROR: login 401";

    fn login_required(r: Result<String, AppError>) {
        match r {
            Err(AppError::User(m)) => assert_eq!(
                m,
                "Entre com sua conta ChatGPT em Configurações → Contas de IA."
            ),
            other => panic!("esperava User, veio {other:?}"),
        }
    }

    fn internal(r: Result<String, AppError>) -> String {
        match r {
            Err(AppError::Internal(m)) => m,
            other => panic!("esperava Internal, veio {other:?}"),
        }
    }

    #[tokio::test]
    async fn exec_unauthorized_asks_for_login() {
        let tmp = tempfile::tempdir().unwrap();
        let llm = failing_codex(
            tmp.path(),
            true,
            "echo 'ERROR: unexpected status 401 Unauthorized: Missing bearer' >&2; exit 1",
        );
        login_required(llm.complete("S", TRICKY).await);
    }

    #[tokio::test]
    async fn exec_failure_when_logged_out_asks_for_login() {
        let tmp = tempfile::tempdir().unwrap();
        let llm = failing_codex(
            tmp.path(),
            false,
            "echo 'ERROR: stream disconnected' >&2; exit 1",
        );
        login_required(llm.complete("S", "U").await);
    }

    #[tokio::test]
    async fn exec_failure_ignores_login_words_in_echoed_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let llm = failing_codex(
            tmp.path(),
            true,
            "echo '2026-11-24T16:31:07Z ERROR codex_api: x' >&2\n\
             echo 'ERROR: stream disconnected before completion' >&2; exit 1",
        );
        let m = internal(llm.complete("S", TRICKY).await);
        assert_eq!(
            m,
            "Codex falhou: ERROR: stream disconnected before completion"
        );
    }

    #[tokio::test]
    async fn exec_failure_without_error_lines_does_not_leak_transcript() {
        let tmp = tempfile::tempdir().unwrap();
        let llm = failing_codex(tmp.path(), true, "exit 2");
        let m = internal(llm.complete("S", TRICKY).await);
        assert_eq!(m, "Codex falhou (código 2).");
    }

    #[test]
    fn auth_status_serializes_state_and_detail() {
        let v = serde_json::to_value(AuthStatus::LoggedIn("x".into())).unwrap();
        assert_eq!(v, serde_json::json!({"state":"logged_in","detail":"x"}));
        let v = serde_json::to_value(AuthStatus::Missing).unwrap();
        assert_eq!(v, serde_json::json!({"state":"missing","detail":null}));
        let v = serde_json::to_value(AuthStatus::LoggedOut).unwrap();
        assert_eq!(v["state"], "logged_out");
    }
}

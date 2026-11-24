//! Execução de programas externos (Codex, Claude Code) com stdin em pipe e timeout.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Roda `program` com `args`/`envs`, escreve `stdin` (se houver) e fecha o pipe, lendo
/// stdout e stderr ao mesmo tempo. No timeout, mata o processo. `label` ("Codex",
/// "Claude Code") é o nome mostrado nas mensagens de erro.
pub async fn run(
    label: &str,
    program: &Path,
    args: &[OsString],
    envs: &[(OsString, OsString)],
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<CliOutput, AppError> {
    run_in(label, program, args, envs, None, stdin, timeout).await
}

/// Como [`run`], com o diretório de trabalho `cwd` (o do app quando `None`).
pub async fn run_in(
    label: &str,
    program: &Path,
    args: &[OsString],
    envs: &[(OsString, OsString)],
    cwd: Option<&Path>,
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<CliOutput, AppError> {
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args)
        .envs(envs.iter().map(|(k, v)| (k, v)))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Internal(format!("Não foi possível executar o {label}: {e}")))?;
    let input = child.stdin.take();
    let mut out = child.stdout.take().expect("stdout em pipe");
    let mut err = child.stderr.take().expect("stderr em pipe");

    let work = async {
        let write = async {
            if let (Some(mut pipe), Some(text)) = (input, stdin) {
                match pipe.write_all(text.as_bytes()).await {
                    // O programa pode sair sem ler a entrada; isso não é erro aqui.
                    Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => return Err(e),
                    _ => {}
                }
                // `pipe` sai de escopo aqui e fecha o stdin.
            }
            Ok(())
        };
        let read_out = async {
            let mut buf = Vec::new();
            out.read_to_end(&mut buf).await.map(|_| buf)
        };
        let read_err = async {
            let mut buf = Vec::new();
            err.read_to_end(&mut buf).await.map(|_| buf)
        };
        let (w, o, e) = tokio::join!(write, read_out, read_err);
        w?;
        let (o, e) = (o?, e?);
        let status = child.wait().await?;
        Ok::<_, std::io::Error>(CliOutput {
            code: status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o).into_owned(),
            stderr: String::from_utf8_lossy(&e).into_owned(),
        })
    };

    match tokio::time::timeout(timeout, work).await {
        Ok(res) => Ok(res?),
        Err(_) => {
            let _ = child.kill().await;
            Err(AppError::User(format!(
                "{label} não respondeu em {} s.",
                timeout.as_secs()
            )))
        }
    }
}

/// Últimos `n` caracteres de `s`, sem espaços nas pontas.
pub fn tail(s: &str, n: usize) -> &str {
    let s = s.trim();
    let count = s.chars().count();
    match s.char_indices().nth(count.saturating_sub(n)) {
        Some((start, _)) if count > n => &s[start..],
        _ => s,
    }
}

/// O que vem depois da última cópia do `prompt` em `s`: alguns CLIs (o Codex, por exemplo)
/// ecoam o prompt no stderr, e a transcrição não pode ser lida como mensagem de erro.
pub fn after_echo<'a>(s: &'a str, prompt: &str) -> &'a str {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return s;
    }
    s.rsplit_once(prompt).map_or(s, |(_, after)| after)
}

/// Indica pedido de login numa saída de erro (já sem o prompt ecoado).
pub fn mentions_login(s: &str) -> bool {
    let s = s.to_lowercase();
    ["login", "not logged in", "unauthorized", "401"]
        .iter()
        .any(|k| s.contains(k))
}

/// Falha de um CLI: pede login quando a saída indica isso; senão `Internal` com o fim do stderr
/// (ou do stdout, se o stderr vier vazio). O `prompt` ecoado é ignorado nas duas saídas.
pub fn failure(out: &CliOutput, label: &str, login_message: &str, prompt: &str) -> AppError {
    let stdout = after_echo(&out.stdout, prompt);
    let stderr = after_echo(&out.stderr, prompt);
    if mentions_login(stdout) || mentions_login(stderr) {
        return AppError::User(login_message.to_string());
    }
    let detail = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    if detail.trim().is_empty() {
        return AppError::Internal(format!("{label} falhou (código {}).", out.code));
    }
    AppError::Internal(format!("{label} falhou: {}", tail(detail, 600)))
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// O Claude Code instalado pelo npm é um `claude.cmd`. O `Command` da std (Rust ≥ 1.77)
    /// roda `.cmd`/`.bat` pelo `cmd.exe` escapando os argumentos; este teste confere isso,
    /// inclusive argumento vazio (`--tools ""`) e stdin.
    #[tokio::test]
    async fn runs_cmd_shim_with_args_and_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("claude.cmd");
        std::fs::write(&bin, "@echo off\r\necho 1=[%~1] 2=[%~2] 3=[%~3]\r\nfindstr \"^\"\r\n").unwrap();
        let args: [OsString; 3] = ["--tools".into(), "".into(), "a b, c.".into()];
        let out = run("Claude Code", &bin, &args, &[], Some("entrada\n"), Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(out.code, 0, "{out:?}");
        assert!(out.stdout.contains("1=[--tools] 2=[] 3=[a b, c.]"), "{out:?}");
        assert!(out.stdout.contains("entrada"), "{out:?}");
    }
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Cria um script `#!/bin/sh` executável em `dir`.
    pub(crate) fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let f = dir.join(name);
        std::fs::write(&f, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        f
    }

    #[test]
    fn tail_keeps_last_chars() {
        assert_eq!(tail("  abc  ", 600), "abc");
        assert_eq!(tail("áéíóú", 2), "óú");
    }

    #[test]
    fn failure_ignores_echoed_prompt() {
        let prompt = "S\n\nfaça login, erro 401 Unauthorized\nERROR: login";
        let out = CliOutput {
            code: 2,
            stdout: String::new(),
            stderr: format!("cabeçalho\nuser\n{prompt}\n\nfalha interna\n"),
        };
        match failure(&out, "X", "entre", prompt) {
            AppError::Internal(m) => assert_eq!(m, "X falhou: falha interna"),
            other => panic!("esperava Internal, veio {other:?}"),
        }
        let only_echo = CliOutput {
            stderr: format!("user\n{prompt}\n"),
            ..out
        };
        match failure(&only_echo, "X", "entre", prompt) {
            AppError::Internal(m) => assert_eq!(m, "X falhou (código 2)."),
            other => panic!("esperava Internal, veio {other:?}"),
        }
    }

    #[tokio::test]
    async fn cli_timeout_kills_and_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(tmp.path(), "lento", "sleep 5");
        let started = std::time::Instant::now();
        let err = run("Lento", &bin, &[], &[], None, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(3));
        match err {
            AppError::User(m) => assert_eq!(m, "Lento não respondeu em 0 s."),
            other => panic!("esperava User, veio {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_passes_args_env_stdin_and_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(
            tmp.path(),
            "eco",
            r#"echo "arg=$1 env=$LT_TESTE"; cat; echo "erro" >&2; exit 3"#,
        );
        let out = run(
            "Eco",
            &bin,
            &["a b".into()],
            &[("LT_TESTE".into(), "valor".into())],
            Some("entrada"),
            Duration::from_secs(10),
        )
        .await
        .unwrap();
        assert_eq!(out.code, 3);
        assert_eq!(out.stdout, "arg=a b env=valor\nentrada");
        assert_eq!(out.stderr, "erro\n");
    }

    #[tokio::test]
    async fn run_handles_large_stdout_and_stdin_without_deadlock() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(tmp.path(), "gato", "cat");
        let big = "x".repeat(2_000_000);
        let out = run("Gato", &bin, &[], &[], Some(&big), Duration::from_secs(20))
            .await
            .unwrap();
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout.len(), big.len());
    }

    #[tokio::test]
    async fn run_ignores_child_that_does_not_read_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(tmp.path(), "surdo", "echo ok");
        let big = "x".repeat(2_000_000);
        let out = run("Gato", &bin, &[], &[], Some(&big), Duration::from_secs(20))
            .await
            .unwrap();
        assert_eq!((out.code, out.stdout.as_str()), (0, "ok\n"));
    }
}

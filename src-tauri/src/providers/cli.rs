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
/// stdout e stderr ao mesmo tempo. No timeout, mata o processo.
pub async fn run(
    program: &Path,
    args: &[OsString],
    envs: &[(OsString, OsString)],
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<CliOutput, AppError> {
    let name = program
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.display().to_string());
    let mut cmd = Command::new(program);
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
        .map_err(|e| AppError::Internal(format!("Não foi possível executar {name}: {e}")))?;
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
                "{name} não respondeu em {} s.",
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

/// Falha de um CLI: pede login quando a saída indica isso; senão `Internal` com o fim do stderr
/// (ou do stdout, se o stderr vier vazio).
pub fn failure(out: &CliOutput, label: &str, login_message: &str) -> AppError {
    let all = format!("{}\n{}", out.stdout, out.stderr).to_lowercase();
    if ["login", "not logged in", "unauthorized", "401"]
        .iter()
        .any(|k| all.contains(k))
    {
        return AppError::User(login_message.to_string());
    }
    let detail = if out.stderr.trim().is_empty() {
        &out.stdout
    } else {
        &out.stderr
    };
    AppError::Internal(format!("{label} falhou: {}", tail(detail, 600)))
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

    #[tokio::test]
    async fn cli_timeout_kills_and_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(tmp.path(), "lento", "sleep 5");
        let started = std::time::Instant::now();
        let err = run(&bin, &[], &[], None, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(3));
        match err {
            AppError::User(m) => assert!(m.contains("não respondeu"), "{m}"),
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
        let out = run(&bin, &[], &[], Some(&big), Duration::from_secs(20))
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
        let out = run(&bin, &[], &[], Some(&big), Duration::from_secs(20))
            .await
            .unwrap();
        assert_eq!((out.code, out.stdout.as_str()), (0, "ok\n"));
    }
}

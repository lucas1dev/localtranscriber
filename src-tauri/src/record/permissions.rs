// Permissão do microfone. No macOS, um microfone negado não dá erro ao abrir:
// a captura só manda silêncio. Por isso a gravação confere antes de abrir (e,
// na primeira vez, pede e espera a resposta). No Windows não há o que conferir aqui.
// A decisão (`ensure_with`) é testada em todas as plataformas, mas só o macOS a usa.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
use std::time::Duration;

use super::devices::MIC_PERMISSION;
use crate::error::AppError;

/// Quanto esperar a pessoa responder ao pedido de permissão.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// O que o sistema diz sobre o acesso ao microfone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Granted,
    Denied,
    /// Ainda não perguntado: pedir.
    Ask,
}

/// Confere (e, se preciso, pede) o acesso ao microfone. Bloqueante: chamar fora
/// do runtime assíncrono.
pub fn ensure_mic() -> Result<(), AppError> {
    #[cfg(target_os = "macos")]
    {
        ensure_with(mac::status, mac::request, REQUEST_TIMEOUT)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

/// `request` devolve a resposta, ou `None` se ela não veio em `timeout`.
fn ensure_with(
    status: impl FnOnce() -> Access,
    request: impl FnOnce(Duration) -> Option<bool>,
    timeout: Duration,
) -> Result<(), AppError> {
    let allowed = match status() {
        Access::Granted => true,
        Access::Denied => false,
        Access::Ask => request(timeout) == Some(true),
    };
    if allowed {
        Ok(())
    } else {
        Err(AppError::User(MIC_PERMISSION.into()))
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

    use super::Access;

    pub fn classify(status: AVAuthorizationStatus) -> Access {
        match status {
            AVAuthorizationStatus::Authorized => Access::Granted,
            AVAuthorizationStatus::NotDetermined => Access::Ask,
            // Negado, restrito (controle parental, MDM) ou um valor novo.
            _ => Access::Denied,
        }
    }

    pub fn status() -> Access {
        // Sem a constante (não deveria acontecer), deixa a abertura decidir.
        let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
            return Access::Granted;
        };
        classify(unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) })
    }

    /// Mostra o pedido do sistema e espera a resposta. O AVFoundation chama o
    /// bloco numa fila do GCD, sem precisar de run loop nesta thread.
    pub fn request(timeout: Duration) -> Option<bool> {
        let audio = unsafe { AVMediaTypeAudio }?;
        let (tx, rx) = mpsc::channel();
        let handler = RcBlock::new(move |granted: Bool| {
            let _ = tx.send(granted.as_bool());
        });
        unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler) };
        rx.recv_timeout(timeout).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn denied(r: Result<(), AppError>) -> bool {
        matches!(r, Err(AppError::User(m)) if m == MIC_PERMISSION)
    }

    #[test]
    fn granted_opens_without_asking() {
        let asked = Cell::new(false);
        let r = ensure_with(|| Access::Granted, |_| { asked.set(true); Some(true) }, REQUEST_TIMEOUT);
        assert!(r.is_ok());
        assert!(!asked.get());
    }

    #[test]
    fn denied_fails_before_opening_without_asking() {
        let asked = Cell::new(false);
        let r = ensure_with(|| Access::Denied, |_| { asked.set(true); Some(true) }, REQUEST_TIMEOUT);
        assert!(denied(r));
        assert!(!asked.get());
    }

    #[test]
    fn not_determined_asks_and_follows_the_answer() {
        let waited = Cell::new(None);
        let r = ensure_with(|| Access::Ask, |t| { waited.set(Some(t)); Some(true) }, REQUEST_TIMEOUT);
        assert!(r.is_ok());
        assert_eq!(waited.get(), Some(Duration::from_secs(60)));
        assert!(denied(ensure_with(|| Access::Ask, |_| Some(false), REQUEST_TIMEOUT)));
    }

    #[test]
    fn unanswered_request_counts_as_denied() {
        assert!(denied(ensure_with(|| Access::Ask, |_| None, REQUEST_TIMEOUT)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_statuses_map_to_access() {
        use objc2_av_foundation::AVAuthorizationStatus as S;
        assert_eq!(mac::classify(S::Authorized), Access::Granted);
        assert_eq!(mac::classify(S::Denied), Access::Denied);
        assert_eq!(mac::classify(S::Restricted), Access::Denied);
        assert_eq!(mac::classify(S::NotDetermined), Access::Ask);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_status_query_does_not_prompt_or_panic() {
        // Só consulta (não pede): qualquer resposta serve.
        let _ = mac::status();
    }
}

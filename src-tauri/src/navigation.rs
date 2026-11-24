//! A janela principal só navega dentro do próprio app: links externos abrem no
//! navegador pelo frontend, e qualquer outra navegação é recusada aqui.

use tauri::plugin::TauriPlugin;
use tauri::{Manager, Runtime, Url};

/// `url` é do próprio app: `tauri://localhost`, `http(s)://tauri.localhost` ou, em
/// desenvolvimento, a mesma origem de `dev_url`.
pub fn is_app_url(url: &Url, dev_url: Option<&Url>) -> bool {
    let host = url.host_str();
    let app = match url.scheme() {
        "tauri" => host == Some("localhost"),
        "http" | "https" => host == Some("tauri.localhost") && url.port().is_none(),
        _ => false,
    };
    app || dev_url.is_some_and(|dev| same_origin(url, dev))
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

pub fn guard<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("navigation-guard")
        .on_navigation(|webview, url| {
            let config = webview.app_handle().config();
            let dev_url = if tauri::is_dev() { config.build.dev_url.as_ref() } else { None };
            let allowed = is_app_url(url, dev_url);
            if !allowed {
                eprintln!("navegação bloqueada: {url}");
            }
            allowed
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn allows_app_origins() {
        for ok in [
            "tauri://localhost",
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "https://tauri.localhost/js/app.js#x",
        ] {
            assert!(is_app_url(&u(ok), None), "{ok}");
        }
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "https://exemplo.com/",
            "http://localhost:1430/",
            "https://tauri.localhost.evil.com/",
            "tauri://outro",
            "file:///etc/passwd",
            "about:blank",
            "data:text/html,oi",
            "javascript:alert(1)",
        ] {
            assert!(!is_app_url(&u(bad), None), "{bad}");
        }
    }

    #[test]
    fn allows_only_the_dev_server_origin_in_dev() {
        let dev = u("http://127.0.0.1:1430/");
        assert!(is_app_url(&u("http://127.0.0.1:1430/index.html"), Some(&dev)));
        assert!(is_app_url(&u("tauri://localhost/"), Some(&dev)));
        assert!(!is_app_url(&u("http://127.0.0.1:1431/"), Some(&dev)));
        assert!(!is_app_url(&u("https://127.0.0.1:1430/"), Some(&dev)));
        assert!(!is_app_url(&u("https://exemplo.com/"), Some(&dev)));
    }
}

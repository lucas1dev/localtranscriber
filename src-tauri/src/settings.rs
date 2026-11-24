use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::AppError;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub default_model: String,
    pub default_language: String,
    pub summary_provider: Option<String>,
    pub summary_style: String,
    pub claude_code_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_model: "large-v3-turbo".into(),
            default_language: "auto".into(),
            summary_provider: None,
            summary_style: "executive".into(),
            claude_code_enabled: false,
        }
    }
}

pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, s: &Settings) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(s).map_err(|e| AppError::Internal(e.to_string()))?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn missing_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(&tmp.path().join("nope.json")), Settings::default());
        let d = Settings::default();
        assert!(!d.claude_code_enabled);
        assert_eq!(d.default_model, "large-v3-turbo");
        assert_eq!(d.default_language, "auto");
        assert_eq!(d.summary_provider, None);
        assert_eq!(d.summary_style, "executive");
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.json");
        fs::write(&p, "{oops").unwrap();
        assert_eq!(load(&p), Settings::default());
    }

    #[test]
    fn partial_file_fills_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.json");
        fs::write(&p, r#"{"default_language":"pt"}"#).unwrap();
        let s = load(&p);
        assert_eq!(s.default_language, "pt");
        assert_eq!(s.default_model, "large-v3-turbo");
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.json");
        let s = Settings {
            default_model: "small".into(),
            summary_provider: Some("codex".into()),
            claude_code_enabled: true,
            ..Settings::default()
        };
        save(&p, &s).unwrap();
        assert_eq!(load(&p), s);
        assert!(!tmp.path().join("s.json.tmp").exists());
    }
}

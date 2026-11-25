use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

use crate::error::AppError;
use crate::providers::ProviderId;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub default_model: String,
    pub default_language: String,
    #[serde(deserialize_with = "lenient_provider")]
    pub summary_provider: Option<ProviderId>,
    pub summary_style: String,
    pub claude_code_enabled: bool,
    #[serde(deserialize_with = "lenient_record_source")]
    pub record_source: String,
    #[serde(deserialize_with = "lenient_record_mic_id")]
    pub record_mic_id: Option<String>,
    #[serde(deserialize_with = "lenient_record_mic_id")]
    pub record_output_id: Option<String>,
}

/// Um valor desconhecido (ou de tipo errado) vira `None` em vez de invalidar o arquivo todo.
fn lenient_provider<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ProviderId>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}

/// Fonte de gravação inválida (ou de tipo errado) vira `"mic"`.
fn lenient_record_source<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v.as_str() {
        Some(s @ ("mic" | "system" | "both")) => s.to_string(),
        _ => "mic".to_string(),
    })
}

/// Id de microfone ou de saída de som de tipo errado vira `None`.
fn lenient_record_mic_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).ok())
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_model: "large-v3-turbo".into(),
            default_language: "auto".into(),
            summary_provider: None,
            summary_style: "executive".into(),
            claude_code_enabled: false,
            record_source: "mic".into(),
            record_mic_id: None,
            record_output_id: None,
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
            summary_provider: Some(ProviderId::Codex),
            claude_code_enabled: true,
            ..Settings::default()
        };
        save(&p, &s).unwrap();
        assert_eq!(load(&p), s);
        assert!(!tmp.path().join("s.json.tmp").exists());
    }

    #[test]
    fn unknown_summary_provider_becomes_none() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("s.json");
        fs::write(&p, r#"{"default_language":"pt","summary_provider":"antigo"}"#).unwrap();
        let s = load(&p);
        assert_eq!(s.summary_provider, None);
        assert_eq!(s.default_language, "pt");
    }

    #[test]
    fn record_fields_default() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.record_source, "mic");
        assert_eq!(s.record_mic_id, None);
        assert_eq!(s.record_output_id, None);
    }

    #[test]
    fn record_source_invalid_becomes_mic() {
        let s: Settings = serde_json::from_str(r#"{"record_source":"xyz"}"#).unwrap();
        assert_eq!(s.record_source, "mic");
        let s: Settings = serde_json::from_str(r#"{"record_source":"both"}"#).unwrap();
        assert_eq!(s.record_source, "both");
    }

    #[test]
    fn record_mic_id_wrong_type_is_none() {
        let s: Settings =
            serde_json::from_str(r#"{"record_mic_id":5,"default_language":"pt","record_source":"system"}"#).unwrap();
        assert_eq!(s.record_mic_id, None);
        assert_eq!(s.default_language, "pt");
        assert_eq!(s.record_source, "system");
    }

    #[test]
    fn record_output_id_wrong_type_is_none() {
        let s: Settings = serde_json::from_str(
            r#"{"record_output_id":5,"record_mic_id":"m1","default_language":"pt"}"#,
        )
        .unwrap();
        assert_eq!(s.record_output_id, None);
        assert_eq!(s.record_mic_id.as_deref(), Some("m1"));
        assert_eq!(s.default_language, "pt");
        let s: Settings = serde_json::from_str(r#"{"record_output_id":"coreaudio:X"}"#).unwrap();
        assert_eq!(s.record_output_id.as_deref(), Some("coreaudio:X"));
    }
}

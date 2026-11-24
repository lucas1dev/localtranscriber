use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    User(String),
    #[error("{0}")]
    Internal(String),
    #[error("Operação cancelada.")]
    Cancelled,
}

impl AppError {
    pub fn kind(&self) -> &'static str {
        match self {
            AppError::User(_) => "user",
            AppError::Internal(_) => "internal",
            AppError::Cancelled => "cancelled",
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut st = serializer.serialize_struct("AppError", 2)?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_kind_and_message() {
        let v = serde_json::to_value(AppError::User("x".into())).unwrap();
        assert_eq!(v, serde_json::json!({"kind":"user","message":"x"}));
        let v = serde_json::to_value(AppError::Cancelled).unwrap();
        assert_eq!(v["kind"], "cancelled");
        assert_eq!(v["message"], "Operação cancelada.");
    }
}

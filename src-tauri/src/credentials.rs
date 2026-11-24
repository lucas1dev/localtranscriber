use crate::error::AppError;

pub const SERVICE: &str = "local-transcriber-ai";

pub trait SecretStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, AppError>;
    fn set(&self, account: &str, secret: &str) -> Result<(), AppError>;
    fn delete(&self, account: &str) -> Result<(), AppError>;
}

pub struct KeyringStore;

fn keychain_err(e: keyring::Error) -> AppError {
    AppError::Internal(format!("Não foi possível acessar o keychain: {e}"))
}

fn entry(account: &str) -> Result<keyring::Entry, AppError> {
    keyring::Entry::new(SERVICE, account).map_err(keychain_err)
}

impl SecretStore for KeyringStore {
    fn get(&self, account: &str) -> Result<Option<String>, AppError> {
        match entry(account)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_err(e)),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), AppError> {
        entry(account)?.set_password(secret).map_err(keychain_err)
    }

    fn delete(&self, account: &str) -> Result<(), AppError> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_err(e)),
        }
    }
}

#[cfg(test)]
pub struct MemoryStore(std::sync::Mutex<std::collections::HashMap<String, String>>);

#[cfg(test)]
impl MemoryStore {
    pub fn new() -> Self {
        Self(Default::default())
    }
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<String>, AppError> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), AppError> {
        self.0.lock().unwrap().insert(account.into(), secret.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), AppError> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
}

/// Retorna a chave sem espaços nas pontas, exigindo de 8 a 500 caracteres.
pub fn normalize_key(key: &str) -> Result<String, AppError> {
    let key = key.trim();
    if (8..=500).contains(&key.chars().count()) {
        Ok(key.to_string())
    } else {
        Err(AppError::User("A chave deve ter entre 8 e 500 caracteres.".into()))
    }
}

#[cfg(test)]
pub fn validate_key(key: &str) -> Result<(), AppError> {
    normalize_key(key).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_key_bounds() {
        assert!(validate_key("1234567").is_err());
        assert!(validate_key("12345678").is_ok());
        assert!(validate_key(&"x".repeat(500)).is_ok());
        assert!(validate_key(&"x".repeat(501)).is_err());
        assert!(validate_key("  1234567  ").is_err());
        assert!(validate_key("ççççççç").is_err()); // 7 caracteres, 14 bytes
        assert!(validate_key("çççççççç").is_ok());
    }

    #[test]
    fn normalize_key_trims() {
        assert_eq!(normalize_key("  abcdefgh\n").unwrap(), "abcdefgh");
        match normalize_key("short") {
            Err(AppError::User(m)) => assert_eq!(m, "A chave deve ter entre 8 e 500 caracteres."),
            other => panic!("esperava User, veio {other:?}"),
        }
    }

    #[test]
    fn memory_store_roundtrip_and_delete_missing_ok() {
        let s = MemoryStore::new();
        assert_eq!(s.get("openai").unwrap(), None);
        s.set("openai", "sk-12345678").unwrap();
        assert_eq!(s.get("openai").unwrap().as_deref(), Some("sk-12345678"));
        s.delete("openai").unwrap();
        assert_eq!(s.get("openai").unwrap(), None);
        s.delete("openai").unwrap();
    }

    #[test]
    #[ignore = "manual: toca o keychain real; LT_EXPECT_KEY_ACCOUNT=anthropic cargo test -- --ignored"]
    fn keyring_reads_python_saved_key() {
        let Ok(account) = std::env::var("LT_EXPECT_KEY_ACCOUNT") else {
            eprintln!("LT_EXPECT_KEY_ACCOUNT não definida; teste manual ignorado");
            return;
        };
        assert!(KeyringStore.get(&account).unwrap().is_some());
    }
}

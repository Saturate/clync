use age::secrecy::ExposeSecret;
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;

use crate::config::EncryptionConfig;
use crate::secret::{CliSecretProvider, SecretProvider};

pub enum Cipher {
    Age(Keys),
    Passphrase(String),
    Plaintext,
}

impl Cipher {
    pub fn from_config(config: &EncryptionConfig) -> Result<Self> {
        Self::from_config_with_provider(config, &CliSecretProvider)
    }

    pub fn from_config_with_provider(
        config: &EncryptionConfig,
        provider: &dyn SecretProvider,
    ) -> Result<Self> {
        match config {
            EncryptionConfig::None => Ok(Self::Plaintext),
            EncryptionConfig::Passphrase { env_var } => {
                let passphrase = std::env::var(env_var).with_context(|| {
                    format!(
                        "environment variable {env_var} not set. Export it before running clync."
                    )
                })?;
                Ok(Self::Passphrase(passphrase))
            }
            other => {
                let secret_key = provider.read_secret(other)?;
                Ok(Self::Age(Keys::from_secret_key(&secret_key)?))
            }
        }
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::Age(keys) => keys.encrypt(plaintext),
            Self::Passphrase(pass) => passphrase_encrypt(plaintext, pass),
            Self::Plaintext => Ok(plaintext.to_vec()),
        }
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::Age(keys) => keys.decrypt(data),
            Self::Passphrase(pass) => passphrase_decrypt(data, pass),
            Self::Plaintext => Ok(data.to_vec()),
        }
    }

    pub fn encrypt_file(&self, src: &Path, dst: &Path) -> Result<()> {
        let plaintext =
            std::fs::read(src).with_context(|| format!("could not read {}", src.display()))?;
        let encrypted = self.encrypt(&plaintext)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dst, encrypted)?;
        Ok(())
    }

    pub fn decrypt_file(&self, src: &Path) -> Result<Vec<u8>> {
        let data =
            std::fs::read(src).with_context(|| format!("could not read {}", src.display()))?;
        self.decrypt(&data)
    }
}

fn passphrase_encrypt(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let encryptor = age::Encryptor::with_user_passphrase(passphrase.into());
    let mut encrypted = vec![];
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .context("failed to create age writer")?;
    writer.write_all(plaintext)?;
    writer.finish()?;
    Ok(encrypted)
}

fn passphrase_decrypt(data: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let identity = age::scrypt::Identity::new(passphrase.into());
    let decryptor = age::Decryptor::new(data).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut plaintext = vec![];
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))?;
    reader.read_to_end(&mut plaintext)?;
    Ok(plaintext)
}

pub struct Keys {
    identity: age::x25519::Identity,
    recipient: age::x25519::Recipient,
}

impl Keys {
    pub fn from_secret_key(secret_key: &str) -> Result<Self> {
        let identity: age::x25519::Identity = secret_key
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid age secret key: {e}"))?;
        let recipient = identity.to_public();
        Ok(Self {
            identity,
            recipient,
        })
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let recipient: Box<dyn age::Recipient + Send> = Box::new(self.recipient.clone());
        let recipients: Vec<&dyn age::Recipient> = vec![recipient.as_ref()];
        let encryptor = age::Encryptor::with_recipients(recipients.into_iter())
            .expect("recipients list is not empty");
        let mut encrypted = vec![];
        let mut writer = encryptor
            .wrap_output(&mut encrypted)
            .context("failed to create age writer")?;
        writer.write_all(plaintext)?;
        writer.finish()?;
        Ok(encrypted)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let decryptor = age::Decryptor::new(ciphertext).map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut plaintext = vec![];
        let mut reader = decryptor
            .decrypt(std::iter::once(&self.identity as &dyn age::Identity))
            .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))?;
        reader.read_to_end(&mut plaintext)?;
        Ok(plaintext)
    }

    pub fn public_key(&self) -> String {
        self.recipient.to_string()
    }

    pub fn secret_key(&self) -> String {
        self.identity.to_string().expose_secret().to_string()
    }

    pub fn generate() -> Self {
        let identity = age::x25519::Identity::generate();
        let recipient = identity.to_public();
        Self {
            identity,
            recipient,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_roundtrip() {
        let keys = Keys::generate();
        let cipher = Cipher::Age(keys);
        let data = b"hello world, this is a test";
        let encrypted = cipher.encrypt(data).unwrap();
        assert_ne!(&encrypted, data);
        let decrypted = cipher.decrypt(&encrypted).unwrap();
        assert_eq!(&decrypted, data);
    }

    #[test]
    fn plaintext_passthrough() {
        let cipher = Cipher::Plaintext;
        let data = b"hello plaintext";
        let encrypted = cipher.encrypt(data).unwrap();
        assert_eq!(&encrypted, data);
        let decrypted = cipher.decrypt(&encrypted).unwrap();
        assert_eq!(&decrypted, data);
    }

    #[test]
    fn passphrase_roundtrip() {
        let cipher = Cipher::Passphrase("test-password-123".into());
        let data = b"secret message";
        let encrypted = cipher.encrypt(data).unwrap();
        assert_ne!(&encrypted, data);
        let decrypted = cipher.decrypt(&encrypted).unwrap();
        assert_eq!(&decrypted, data);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let cipher1 = Cipher::Passphrase("correct".into());
        let cipher2 = Cipher::Passphrase("wrong".into());
        let data = b"secret";
        let encrypted = cipher1.encrypt(data).unwrap();
        assert!(cipher2.decrypt(&encrypted).is_err());
    }

    #[test]
    fn key_generation() {
        let keys = Keys::generate();
        assert!(keys.public_key().starts_with("age1"));
        assert!(keys.secret_key().starts_with("AGE-SECRET-KEY-"));
    }

    #[test]
    fn from_secret_key_roundtrip() {
        let keys = Keys::generate();
        let secret = keys.secret_key();
        let keys2 = Keys::from_secret_key(&secret).unwrap();
        assert_eq!(keys.public_key(), keys2.public_key());
    }

    #[test]
    fn from_config_with_mock_provider() {
        let keys = Keys::generate();
        let secret = keys.secret_key();
        let provider = crate::secret::MockSecretProvider::new(&secret);
        let config = EncryptionConfig::OnePassword {
            reference: "op://test/test/key".to_string(),
        };
        let cipher = Cipher::from_config_with_provider(&config, &provider).unwrap();
        let data = b"test data";
        let encrypted = cipher.encrypt(data).unwrap();
        let decrypted = cipher.decrypt(&encrypted).unwrap();
        assert_eq!(&decrypted, data);
    }
}

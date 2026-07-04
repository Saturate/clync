use age::secrecy::ExposeSecret;
use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;

use crate::config::EncryptionConfig;

pub enum Cipher {
    Age(Keys),
    Passphrase(String),
    Plaintext,
}

impl Cipher {
    pub fn from_config(config: &EncryptionConfig) -> Result<Self> {
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
            other => Ok(Self::Age(Keys::from_config(other)?)),
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
    pub fn from_config(config: &EncryptionConfig) -> Result<Self> {
        let secret_key = match config {
            EncryptionConfig::KeyFile { path } => std::fs::read_to_string(path)
                .with_context(|| format!("could not read key file: {}", path.display()))?
                .trim()
                .to_string(),
            EncryptionConfig::OnePassword { reference } => {
                run_secret_cmd("op", &["read", reference, "--no-newline"])?
            }
            EncryptionConfig::Bitwarden { item_id, field } => {
                run_secret_cmd("bw", &["get", field, item_id])?
            }
            EncryptionConfig::Pass { entry } => run_secret_cmd("pass", &["show", entry])?,
            EncryptionConfig::Passphrase { .. } | EncryptionConfig::None => {
                bail!("cannot create key-based cipher for this encryption mode")
            }
        };

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

fn run_secret_cmd(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `{program}`. Is it installed?"))?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)
        .context("command returned non-UTF8 data")?
        .trim()
        .to_string())
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
}

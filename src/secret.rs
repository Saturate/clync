use anyhow::{Context, Result, bail};
use std::process::Command;

use crate::config::EncryptionConfig;

pub trait SecretProvider: Send + Sync {
    fn read_secret(&self, config: &EncryptionConfig) -> Result<String>;
}

pub struct CliSecretProvider;

impl SecretProvider for CliSecretProvider {
    fn read_secret(&self, config: &EncryptionConfig) -> Result<String> {
        match config {
            EncryptionConfig::KeyFile { path } => std::fs::read_to_string(path)
                .with_context(|| format!("could not read key file: {}", path.display()))
                .map(|s| s.trim().to_string()),
            EncryptionConfig::OnePassword { reference } => {
                run_secret_cmd("op", &["read", reference, "--no-newline"])
            }
            EncryptionConfig::Bitwarden { item_id, field } => {
                run_secret_cmd("bw", &["get", field, item_id])
            }
            EncryptionConfig::Pass { entry } => run_secret_cmd("pass", &["show", entry]),
            EncryptionConfig::Passphrase { .. } | EncryptionConfig::None => {
                bail!("cannot read secret for this encryption mode")
            }
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
pub struct MockSecretProvider {
    secret: String,
}

#[cfg(test)]
impl MockSecretProvider {
    pub fn new(secret: &str) -> Self {
        Self {
            secret: secret.to_string(),
        }
    }
}

#[cfg(test)]
impl SecretProvider for MockSecretProvider {
    fn read_secret(&self, _config: &EncryptionConfig) -> Result<String> {
        Ok(self.secret.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn mock_provider_returns_secret() {
        let provider = MockSecretProvider::new("AGE-SECRET-KEY-TEST");
        let config = EncryptionConfig::KeyFile {
            path: PathBuf::from("/fake"),
        };
        assert_eq!(
            provider.read_secret(&config).unwrap(),
            "AGE-SECRET-KEY-TEST"
        );
    }

    #[test]
    fn cli_provider_key_file_not_found() {
        let provider = CliSecretProvider;
        let config = EncryptionConfig::KeyFile {
            path: PathBuf::from("/nonexistent/key.txt"),
        };
        assert!(provider.read_secret(&config).is_err());
    }

    #[test]
    fn cli_provider_passphrase_errors() {
        let provider = CliSecretProvider;
        let config = EncryptionConfig::Passphrase {
            env_var: "UNUSED".into(),
        };
        assert!(provider.read_secret(&config).is_err());
    }

    #[test]
    fn cli_provider_none_errors() {
        let provider = CliSecretProvider;
        let config = EncryptionConfig::None;
        assert!(provider.read_secret(&config).is_err());
    }

    #[test]
    fn cli_provider_reads_real_key_file() {
        let dir = std::env::temp_dir().join(format!("clync-secret-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("test-key.txt");
        std::fs::write(&key_path, "my-secret-key\n").unwrap();

        let provider = CliSecretProvider;
        let config = EncryptionConfig::KeyFile { path: key_path };
        assert_eq!(provider.read_secret(&config).unwrap(), "my-secret-key");

        std::fs::remove_dir_all(&dir).ok();
    }
}

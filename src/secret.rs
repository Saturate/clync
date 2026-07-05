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

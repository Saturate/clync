use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoMeta {
    pub version: u32,
    pub encryption: EncryptionMeta,
    #[serde(default)]
    pub targets: TargetsMeta,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EncryptionMeta {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TargetsMeta {
    pub sessions: bool,
    pub memories: bool,
    pub settings: bool,
    pub commands: bool,
    pub skills: bool,
    pub global_claude_md: bool,
}

#[allow(dead_code)]
impl RepoMeta {
    pub fn file_extension(&self) -> &str {
        match self.encryption.method.as_str() {
            "none" => "jsonl",
            _ => "age",
        }
    }

    pub fn load(repo_path: &Path) -> Result<Option<Self>> {
        let path = repo_path.join("clync.toml");
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read {}", path.display()))?;
        let meta: RepoMeta = toml::from_str(&contents).context("invalid clync.toml")?;
        Ok(Some(meta))
    }

    pub fn save(&self, repo_path: &Path) -> Result<()> {
        let path = repo_path.join("clync.toml");
        let contents = toml::to_string_pretty(self)?;
        std::fs::write(&path, contents)?;
        Ok(())
    }

    pub fn from_config(config: &crate::config::Config) -> Self {
        let (method, hint) = match &config.encryption {
            crate::config::EncryptionConfig::KeyFile { .. } => ("key_file".into(), None),
            crate::config::EncryptionConfig::Passphrase { .. } => ("passphrase".into(), None),
            crate::config::EncryptionConfig::OnePassword { reference } => {
                ("onepassword".into(), Some(reference.clone()))
            }
            crate::config::EncryptionConfig::Bitwarden { item_id, .. } => {
                ("bitwarden".into(), Some(item_id.clone()))
            }
            crate::config::EncryptionConfig::Pass { entry } => ("pass".into(), Some(entry.clone())),
            crate::config::EncryptionConfig::None => ("none".into(), None),
        };

        let t = &config.targets;
        RepoMeta {
            version: 1,
            encryption: EncryptionMeta { method, hint },
            targets: TargetsMeta {
                sessions: t.sessions,
                memories: t.memories,
                settings: t.settings,
                commands: t.commands,
                skills: t.skills,
                global_claude_md: t.global_claude_md,
            },
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use std::path::PathBuf;

    #[test]
    fn file_extension_none() {
        let meta = RepoMeta {
            version: 1,
            encryption: EncryptionMeta {
                method: "none".into(),
                hint: None,
            },
            targets: TargetsMeta::default(),
        };
        assert_eq!(meta.file_extension(), "jsonl");
    }

    #[test]
    fn file_extension_age() {
        let meta = RepoMeta {
            version: 1,
            encryption: EncryptionMeta {
                method: "key_file".into(),
                hint: None,
            },
            targets: TargetsMeta::default(),
        };
        assert_eq!(meta.file_extension(), "age");
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("clync-meta-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let meta = RepoMeta {
            version: 1,
            encryption: EncryptionMeta {
                method: "none".into(),
                hint: None,
            },
            targets: TargetsMeta {
                sessions: true,
                memories: true,
                settings: false,
                commands: false,
                skills: false,
                global_claude_md: false,
            },
        };
        meta.save(&dir).unwrap();

        let loaded = RepoMeta::load(&dir).unwrap().unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.encryption.method, "none");
        assert!(loaded.targets.sessions);
        assert!(!loaded.targets.settings);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_missing_returns_none() {
        let result = RepoMeta::load(std::path::Path::new("/nonexistent")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn from_config_onepassword() {
        let config = Config {
            sync: SyncConfig {
                repo: PathBuf::from("/tmp"),
                claude_dir: PathBuf::from("/tmp"),
                include_companion_dirs: false,
                auto_git: true,
                git: Default::default(),
            },
            encryption: EncryptionConfig::OnePassword {
                reference: "op://vault/item".into(),
            },
            targets: SyncTargets::default(),
        };
        let meta = RepoMeta::from_config(&config);
        assert_eq!(meta.encryption.method, "onepassword");
        assert_eq!(meta.encryption.hint.unwrap(), "op://vault/item");
    }

    #[test]
    fn from_config_bitwarden() {
        let config = Config {
            sync: SyncConfig {
                repo: PathBuf::from("/tmp"),
                claude_dir: PathBuf::from("/tmp"),
                include_companion_dirs: false,
                auto_git: true,
                git: Default::default(),
            },
            encryption: EncryptionConfig::Bitwarden {
                item_id: "my-item".into(),
                field: "notes".into(),
            },
            targets: SyncTargets::default(),
        };
        let meta = RepoMeta::from_config(&config);
        assert_eq!(meta.encryption.method, "bitwarden");
        assert_eq!(meta.encryption.hint.unwrap(), "my-item");
    }

    #[test]
    fn from_config_pass() {
        let config = Config {
            sync: SyncConfig {
                repo: PathBuf::from("/tmp"),
                claude_dir: PathBuf::from("/tmp"),
                include_companion_dirs: false,
                auto_git: true,
                git: Default::default(),
            },
            encryption: EncryptionConfig::Pass {
                entry: "clync/key".into(),
            },
            targets: SyncTargets::default(),
        };
        let meta = RepoMeta::from_config(&config);
        assert_eq!(meta.encryption.method, "pass");
        assert_eq!(meta.encryption.hint.unwrap(), "clync/key");
    }

    #[test]
    fn from_config_passphrase() {
        let config = Config {
            sync: SyncConfig {
                repo: PathBuf::from("/tmp"),
                claude_dir: PathBuf::from("/tmp"),
                include_companion_dirs: false,
                auto_git: true,
                git: Default::default(),
            },
            encryption: EncryptionConfig::Passphrase {
                env_var: "MY_PASS".into(),
            },
            targets: SyncTargets::default(),
        };
        let meta = RepoMeta::from_config(&config);
        assert_eq!(meta.encryption.method, "passphrase");
        assert!(meta.encryption.hint.is_none());
    }
}

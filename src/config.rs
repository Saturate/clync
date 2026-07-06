use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub sync: SyncConfig,
    pub encryption: EncryptionConfig,
    #[serde(default)]
    pub targets: SyncTargets,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncConfig {
    pub repo: PathBuf,
    pub claude_dir: PathBuf,
    #[serde(default)]
    pub include_companion_dirs: bool,
    #[serde(default = "default_true")]
    pub auto_git: bool,
    #[serde(default)]
    pub git: GitConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default = "default_lfs_threshold")]
    pub lfs_threshold: u64,
}

fn default_lfs_threshold() -> u64 {
    99 * 1024 * 1024
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            lfs_threshold: default_lfs_threshold(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncTargets {
    #[serde(default = "default_true")]
    pub sessions: bool,
    #[serde(default = "default_true")]
    pub memories: bool,
    #[serde(default = "default_true")]
    pub settings: bool,
    #[serde(default = "default_true")]
    pub commands: bool,
    #[serde(default = "default_true")]
    pub skills: bool,
    #[serde(default = "default_true")]
    pub global_claude_md: bool,
}

impl Default for SyncTargets {
    fn default() -> Self {
        Self {
            sessions: true,
            memories: true,
            settings: true,
            commands: true,
            skills: true,
            global_claude_md: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum EncryptionConfig {
    #[serde(rename = "key_file")]
    KeyFile { path: PathBuf },
    #[serde(rename = "passphrase")]
    Passphrase { env_var: String },
    #[serde(rename = "onepassword")]
    OnePassword { reference: String },
    #[serde(rename = "bitwarden")]
    Bitwarden { item_id: String, field: String },
    #[serde(rename = "pass")]
    Pass { entry: String },
    #[serde(rename = "none")]
    None,
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(xdg).join("clync"));
        }
        let home = home_dir().context("could not determine home directory")?;
        Ok(home.join(".config").join("clync"))
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("could not read config at {}", path.display()))?;
        let mut config: Config = toml::from_str(&contents).context("invalid config format")?;

        config.sync.repo = expand_path(&config.sync.repo);
        config.sync.claude_dir = expand_path(&config.sync.claude_dir);
        if let EncryptionConfig::KeyFile { ref mut path } = config.encryption {
            *path = expand_path(path);
        }

        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let contents = toml::to_string_pretty(self).context("failed to serialize config")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, contents)?;
        Ok(())
    }

    pub fn claude_projects_dir(&self) -> PathBuf {
        self.sync.claude_dir.join("projects")
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

pub fn expand_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.starts_with("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(&s[2..]);
    }
    p.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_path_no_tilde() {
        let p = PathBuf::from("/usr/local/bin");
        assert_eq!(expand_path(&p), p);
    }

    #[test]
    fn expand_path_with_tilde() {
        let result = expand_path(&PathBuf::from("~/test/path"));
        assert!(!result.to_string_lossy().starts_with("~/"));
        assert!(result.to_string_lossy().ends_with("test/path"));
    }

    #[test]
    fn expand_path_relative() {
        let p = PathBuf::from("relative/path");
        assert_eq!(expand_path(&p), p);
    }

    #[test]
    fn home_dir_returns_some() {
        assert!(home_dir().is_some());
    }

    #[test]
    fn sync_targets_default_all_true() {
        let targets = SyncTargets::default();
        assert!(targets.sessions);
        assert!(targets.memories);
        assert!(targets.settings);
        assert!(targets.commands);
        assert!(targets.skills);
        assert!(targets.global_claude_md);
    }

    #[test]
    fn config_roundtrip() {
        let dir = std::env::temp_dir().join(format!("clync-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config {
            sync: SyncConfig {
                repo: PathBuf::from("/tmp/repo"),
                claude_dir: PathBuf::from("/home/user/.claude"),
                include_companion_dirs: false,
                auto_git: true,
                git: Default::default(),
            },
            encryption: EncryptionConfig::None,
            targets: SyncTargets::default(),
        };

        config.save(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let loaded: Config = toml::from_str(&contents).unwrap();
        assert!(matches!(loaded.encryption, EncryptionConfig::None));
        assert!(loaded.targets.sessions);

        std::fs::remove_dir_all(&dir).ok();
    }
}

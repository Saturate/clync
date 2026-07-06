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
    pub claude_dir: PathBuf,
    #[serde(default)]
    pub include_companion_dirs: bool,
    pub storage: StorageConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StorageConfig {
    #[serde(rename = "git")]
    Git {
        path: PathBuf,
        #[serde(default = "default_true")]
        auto_push: bool,
        #[serde(default = "default_lfs_threshold")]
        lfs_threshold: u64,
    },
    #[serde(rename = "folder")]
    Folder { path: PathBuf },
    #[cfg(feature = "s3")]
    #[serde(rename = "s3")]
    S3 {
        bucket: String,
        #[serde(default)]
        prefix: String,
        region: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access_key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_key: Option<String>,
    },
}

impl std::fmt::Debug for StorageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageConfig::Git {
                path,
                auto_push,
                lfs_threshold,
            } => f
                .debug_struct("Git")
                .field("path", path)
                .field("auto_push", auto_push)
                .field("lfs_threshold", lfs_threshold)
                .finish(),
            StorageConfig::Folder { path } => f.debug_struct("Folder").field("path", path).finish(),
            #[cfg(feature = "s3")]
            StorageConfig::S3 {
                bucket,
                prefix,
                region,
                endpoint,
                ..
            } => f
                .debug_struct("S3")
                .field("bucket", bucket)
                .field("prefix", prefix)
                .field("region", region)
                .field("endpoint", endpoint)
                .field("access_key", &"[REDACTED]")
                .field("secret_key", &"[REDACTED]")
                .finish(),
        }
    }
}

impl StorageConfig {
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            StorageConfig::Git { path, .. } => Some(path),
            StorageConfig::Folder { path } => Some(path),
            #[cfg(feature = "s3")]
            StorageConfig::S3 { .. } => None,
        }
    }

    pub fn is_git(&self) -> bool {
        matches!(self, StorageConfig::Git { .. })
    }

    pub fn auto_push(&self) -> bool {
        match self {
            StorageConfig::Git { auto_push, .. } => *auto_push,
            _ => false,
        }
    }

    pub fn lfs_threshold(&self) -> u64 {
        match self {
            StorageConfig::Git { lfs_threshold, .. } => *lfs_threshold,
            _ => 0,
        }
    }
}

pub fn default_lfs_threshold() -> u64 {
    99 * 1024 * 1024
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

        match &mut config.sync.storage {
            StorageConfig::Git { path, .. } | StorageConfig::Folder { path } => {
                *path = expand_path(path);
            }
            #[cfg(feature = "s3")]
            StorageConfig::S3 { .. } => {}
        }
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

    pub fn storage_path(&self) -> Option<&Path> {
        self.sync.storage.local_path()
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
                claude_dir: PathBuf::from("/home/user/.claude"),
                include_companion_dirs: false,
                storage: StorageConfig::Git {
                    path: PathBuf::from("/tmp/repo"),
                    auto_push: true,
                    lfs_threshold: default_lfs_threshold(),
                },
            },
            encryption: EncryptionConfig::None,
            targets: SyncTargets::default(),
        };

        config.save(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let loaded: Config = toml::from_str(&contents).unwrap();
        assert!(matches!(loaded.encryption, EncryptionConfig::None));
        assert!(loaded.targets.sessions);
        assert!(loaded.sync.storage.is_git());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn folder_config_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("clync-folder-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let config = Config {
            sync: SyncConfig {
                claude_dir: PathBuf::from("/home/user/.claude"),
                include_companion_dirs: false,
                storage: StorageConfig::Folder {
                    path: PathBuf::from("/mnt/nas/clync"),
                },
            },
            encryption: EncryptionConfig::None,
            targets: SyncTargets::default(),
        };

        config.save(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let loaded: Config = toml::from_str(&contents).unwrap();
        assert!(matches!(loaded.sync.storage, StorageConfig::Folder { .. }));
        assert!(!loaded.sync.storage.is_git());
        assert!(!loaded.sync.storage.auto_push());

        std::fs::remove_dir_all(&dir).ok();
    }
}

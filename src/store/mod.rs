pub mod folder;
pub mod git;
#[cfg(feature = "s3")]
pub mod s3;

use anyhow::{Context, Result};
use fs2::FileExt;
use std::path::{Path, PathBuf};

use crate::config::{Config, StorageConfig};

#[allow(dead_code)]
pub trait Store: Send + Sync {
    fn write_file(&self, rel_path: &str, data: &[u8]) -> Result<()>;
    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>>;
    fn exists(&self, rel_path: &str) -> bool;
    fn list_files(&self, prefix: &str) -> Result<Vec<String>>;
    fn file_size(&self, rel_path: &str) -> Result<u64>;
    fn atomic_write(&self, rel_path: &str, data: &[u8]) -> Result<()>;

    fn sync_down(&self) -> Result<()>;
    fn sync_up(&self, message: &str) -> Result<()>;

    fn lock(&self) -> Result<Box<dyn std::any::Any>>;
    fn try_lock(&self) -> Result<Box<dyn std::any::Any>>;

    fn local_path(&self) -> Option<&Path>;
}

pub struct LocalFs {
    root: PathBuf,
}

#[allow(dead_code)]
impl LocalFs {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_file(&self, rel_path: &str, data: &[u8]) -> Result<()> {
        let full_path = self.root.join(rel_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, data)?;
        Ok(())
    }

    pub fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
        let full_path = self.root.join(rel_path);
        std::fs::read(&full_path).with_context(|| format!("could not read {}", full_path.display()))
    }

    pub fn exists(&self, rel_path: &str) -> bool {
        self.root.join(rel_path).exists()
    }

    pub fn list_files(&self, prefix: &str) -> Result<Vec<String>> {
        let dir = self.root.join(prefix);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && let Some(name) = entry.file_name().to_str()
            {
                files.push(format!("{prefix}/{name}"));
            }
        }
        Ok(files)
    }

    pub fn file_size(&self, rel_path: &str) -> Result<u64> {
        let full_path = self.root.join(rel_path);
        Ok(std::fs::metadata(&full_path)
            .with_context(|| format!("could not stat {}", full_path.display()))?
            .len())
    }

    pub fn atomic_write(&self, rel_path: &str, data: &[u8]) -> Result<()> {
        let full_path = self.root.join(rel_path);
        let tmp_path = self.root.join(format!("{rel_path}.tmp"));
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&tmp_path, data)?;
        std::fs::rename(&tmp_path, &full_path)?;
        Ok(())
    }

    pub fn lock(&self) -> Result<Box<dyn std::any::Any>> {
        self.lock_inner(true)
    }

    pub fn try_lock(&self) -> Result<Box<dyn std::any::Any>> {
        self.lock_inner(false)
    }

    fn lock_inner(&self, blocking: bool) -> Result<Box<dyn std::any::Any>> {
        let lock_path = self.root.join(".clync.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .context("could not create lock file")?;
        if blocking {
            file.lock_exclusive()
                .context("another clync process is running (locked)")?;
        } else {
            file.try_lock_exclusive()
                .context("sync in progress, try again shortly")?;
        }
        Ok(Box::new(file))
    }
}

pub fn create_store(config: &Config) -> Result<Box<dyn Store>> {
    match &config.sync.storage {
        StorageConfig::Git { path, .. } => Ok(Box::new(git::GitStore::new(path.clone()))),
        StorageConfig::Folder { path } => Ok(Box::new(folder::FolderStore::new(path.clone()))),
        #[cfg(feature = "s3")]
        StorageConfig::S3 {
            bucket,
            prefix,
            region,
            endpoint,
            access_key,
            secret_key,
        } => Ok(Box::new(s3::S3Store::new(
            bucket.clone(),
            prefix.clone(),
            region.clone(),
            endpoint.clone(),
            access_key.clone(),
            secret_key.clone(),
        )?)),
    }
}

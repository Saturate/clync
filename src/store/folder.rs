use anyhow::Result;
use std::path::{Path, PathBuf};

use super::{LocalFs, Store};

pub struct FolderStore {
    fs: LocalFs,
}

impl FolderStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            fs: LocalFs::new(path),
        }
    }

    #[allow(dead_code)]
    pub fn init(path: &Path) -> Result<Self> {
        std::fs::create_dir_all(path)?;
        Ok(Self::new(path.to_path_buf()))
    }

    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        self.fs.root()
    }
}

impl Store for FolderStore {
    fn write_file(&self, rel_path: &str, data: &[u8]) -> Result<()> {
        self.fs.write_file(rel_path, data)
    }

    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
        self.fs.read_file(rel_path)
    }

    fn exists(&self, rel_path: &str) -> bool {
        self.fs.exists(rel_path)
    }

    fn list_files(&self, prefix: &str) -> Result<Vec<String>> {
        self.fs.list_files(prefix)
    }

    fn file_size(&self, rel_path: &str) -> Result<u64> {
        self.fs.file_size(rel_path)
    }

    fn atomic_write(&self, rel_path: &str, data: &[u8]) -> Result<()> {
        self.fs.atomic_write(rel_path, data)
    }

    fn sync_down(&self) -> Result<()> {
        Ok(())
    }

    fn sync_up(&self, _message: &str) -> Result<()> {
        Ok(())
    }

    fn lock(&self) -> Result<Box<dyn std::any::Any>> {
        self.fs.lock()
    }

    fn try_lock(&self) -> Result<Box<dyn std::any::Any>> {
        self.fs.try_lock()
    }

    fn local_path(&self) -> Option<&Path> {
        Some(self.fs.root())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("clync-folder-store-test")
            .join(name)
            .join(format!("{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn init_creates_dir() {
        let dir = temp_dir("init");
        let path = dir.join("store");
        let store = FolderStore::init(&path).unwrap();
        assert!(path.exists());
        assert_eq!(store.root(), path);
        assert!(!path.join(".git").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_read_exists() {
        let dir = temp_dir("write_read");
        let store = FolderStore::init(&dir.join("store")).unwrap();
        store.write_file("test.txt", b"hello").unwrap();
        assert!(store.exists("test.txt"));
        assert!(!store.exists("missing.txt"));
        let data = store.read_file("test.txt").unwrap();
        assert_eq!(data, b"hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_files_works() {
        let dir = temp_dir("list_files");
        let store = FolderStore::init(&dir.join("store")).unwrap();
        store.write_file("sessions/a.txt", b"a").unwrap();
        store.write_file("sessions/b.txt", b"b").unwrap();
        let files = store.list_files("sessions").unwrap();
        assert_eq!(files.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sync_up_down_are_noops() {
        let dir = temp_dir("sync_noop");
        let store = FolderStore::init(&dir.join("store")).unwrap();
        store.sync_up("test").unwrap();
        store.sync_down().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lock_works() {
        let dir = temp_dir("lock");
        let store = FolderStore::init(&dir.join("store")).unwrap();
        let _lock = store.lock().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn atomic_write_works() {
        let dir = temp_dir("atomic");
        let store = FolderStore::init(&dir.join("store")).unwrap();
        store.atomic_write("test.txt", b"data").unwrap();
        assert_eq!(store.read_file("test.txt").unwrap(), b"data");
        assert!(!store.exists("test.txt.tmp"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_size_works() {
        let dir = temp_dir("file_size");
        let store = FolderStore::init(&dir.join("store")).unwrap();
        store.write_file("test.txt", b"12345").unwrap();
        assert_eq!(store.file_size("test.txt").unwrap(), 5);
        std::fs::remove_dir_all(&dir).ok();
    }
}

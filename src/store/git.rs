use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use super::{LocalFs, Store};

pub struct GitStore {
    fs: LocalFs,
}

impl GitStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            fs: LocalFs::new(path),
        }
    }

    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        self.fs.root()
    }

    pub fn init_repo(repo_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(repo_path)?;
        if !repo_path.join(".git").exists() {
            let status = std::process::Command::new("git")
                .args(["init", "-b", "main"])
                .current_dir(repo_path)
                .status()
                .context("git init failed")?;
            if !status.success() {
                bail!("git init failed");
            }
            std::fs::write(repo_path.join(".gitignore"), ".clync.lock\n")?;
        }
        Ok(Self::new(repo_path.to_path_buf()))
    }

    pub fn clone_repo(url: &str, dest: &Path) -> Result<Self> {
        let status = std::process::Command::new("git")
            .args(["clone", url, &dest.to_string_lossy()])
            .status()
            .context("git clone failed")?;
        if !status.success() {
            bail!("git clone failed");
        }
        Ok(Self::new(dest.to_path_buf()))
    }

    pub fn has_remote(&self) -> bool {
        std::process::Command::new("git")
            .args(["remote"])
            .current_dir(self.fs.root())
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    }

    pub fn get_remote_url(&self) -> Option<String> {
        std::process::Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(self.fs.root())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    pub fn add_remote(&self, url: &str) -> Result<()> {
        self.run_git(&["remote", "add", "origin", url])
    }

    pub fn checkout_first_branch(&self) -> Result<()> {
        let branches = std::process::Command::new("git")
            .args(["branch", "-r"])
            .current_dir(self.fs.root())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        for line in branches.lines() {
            let branch = line.trim().trim_start_matches("origin/");
            if branch == "HEAD" || branch.contains("->") {
                continue;
            }
            let _ = std::process::Command::new("git")
                .args(["checkout", branch])
                .current_dir(self.fs.root())
                .status();
            break;
        }
        Ok(())
    }

    pub fn commit(&self, message: &str) -> Result<()> {
        self.run_git(&["add", "-A"])?;
        let status = std::process::Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(self.fs.root())
            .status()?;
        if status.success() {
            return Ok(());
        }
        self.run_git(&["commit", "-m", message])
    }

    pub fn push_remote(&self) -> Result<()> {
        if !self.has_remote() {
            return Ok(());
        }
        let result = self.run_git(&["push"]);
        if result.is_err() {
            let branch = std::process::Command::new("git")
                .args(["branch", "--show-current"])
                .current_dir(self.fs.root())
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| "main".into());
            self.run_git(&["push", "--set-upstream", "origin", &branch])?;
        }
        Ok(())
    }

    pub fn pull_remote(&self) -> Result<()> {
        if !self.has_remote() {
            return Ok(());
        }
        let output = std::process::Command::new("git")
            .args(["pull", "--ff-only"])
            .current_dir(self.fs.root())
            .output()
            .context("git pull failed")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Not possible to fast-forward") || stderr.contains("divergent") {
                bail!(
                    "git pull failed: repos have diverged. \
                     Try: cd {} && git pull --rebase",
                    self.fs.root().display()
                );
            }
            bail!("git pull --ff-only failed: {}", stderr.trim());
        }
        Ok(())
    }

    fn run_git(&self, args: &[&str]) -> Result<()> {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(self.fs.root())
            .status()
            .with_context(|| format!("git {} failed", args.join(" ")))?;
        if !status.success() {
            bail!("git {} exited with {}", args.join(" "), status);
        }
        Ok(())
    }
}

impl Store for GitStore {
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
        self.pull_remote()
    }

    fn sync_up(&self, message: &str) -> Result<()> {
        self.commit(message)?;
        self.push_remote()
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
            .join("clync-git-store-test")
            .join(name)
            .join(format!("{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn init_repo_creates_git_dir() {
        let dir = temp_dir("init_repo");
        let repo_path = dir.join("repo");
        let store = GitStore::init_repo(&repo_path).unwrap();
        assert!(repo_path.join(".git").exists());
        assert!(repo_path.join(".gitignore").exists());
        assert_eq!(store.root(), repo_path);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn init_repo_idempotent() {
        let dir = temp_dir("init_idem");
        let repo_path = dir.join("repo");
        GitStore::init_repo(&repo_path).unwrap();
        GitStore::init_repo(&repo_path).unwrap();
        assert!(repo_path.join(".git").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_read_exists() {
        let dir = temp_dir("write_read");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        store.write_file("test.txt", b"hello").unwrap();
        assert!(store.exists("test.txt"));
        assert!(!store.exists("missing.txt"));
        let data = store.read_file("test.txt").unwrap();
        assert_eq!(data, b"hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_creates_subdirs() {
        let dir = temp_dir("subdirs");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        store.write_file("a/b/c.txt", b"nested").unwrap();
        assert!(store.exists("a/b/c.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_files_works() {
        let dir = temp_dir("list_files");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        store.write_file("sessions/a.txt", b"a").unwrap();
        store.write_file("sessions/b.txt", b"b").unwrap();
        let files = store.list_files("sessions").unwrap();
        assert_eq!(files.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_files_empty() {
        let dir = temp_dir("list_empty");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        let files = store.list_files("nonexistent").unwrap();
        assert!(files.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn has_remote_false_for_new_repo() {
        let dir = temp_dir("no_remote");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        assert!(!store.has_remote());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_remote_url_none_for_new_repo() {
        let dir = temp_dir("no_remote_url");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        assert!(store.get_remote_url().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_remote_and_check() {
        let dir = temp_dir("add_remote");
        let bare = dir.join("bare.git");
        std::process::Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(&bare)
            .output()
            .unwrap();
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        store.add_remote(bare.to_str().unwrap()).unwrap();
        assert!(store.has_remote());
        assert!(store.get_remote_url().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lock_and_try_lock() {
        let dir = temp_dir("lock");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        let _lock = store.lock().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    fn set_git_test_author(repo: &Path) {
        for args in [
            &["config", "user.email", "test@clync"][..],
            &["config", "user.name", "test"],
            &["config", "commit.gpgsign", "false"],
        ] {
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .output()
                .unwrap();
        }
    }

    #[test]
    fn commit_with_changes() {
        let dir = temp_dir("commit");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        set_git_test_author(store.root());
        store.write_file("file.txt", b"content").unwrap();
        store.commit("test commit").unwrap();

        let log = std::process::Command::new("git")
            .args(["log", "--oneline"])
            .current_dir(store.root())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&log.stdout);
        assert!(stdout.contains("test commit"), "log: {stdout}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_no_changes_is_noop() {
        let dir = temp_dir("commit_noop");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        set_git_test_author(store.root());
        store.write_file("file.txt", b"content").unwrap();
        store.commit("first").unwrap();
        store.commit("second should be noop").unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pull_remote_no_remote_is_ok() {
        let dir = temp_dir("pull_no_remote");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        store.pull_remote().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn push_remote_no_remote_is_ok() {
        let dir = temp_dir("push_no_remote");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        store.push_remote().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_size_works() {
        let dir = temp_dir("file_size");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        store.write_file("test.txt", b"hello").unwrap();
        assert_eq!(store.file_size("test.txt").unwrap(), 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn atomic_write_works() {
        let dir = temp_dir("atomic_write");
        let store = GitStore::init_repo(&dir.join("repo")).unwrap();
        store.atomic_write("test.txt", b"atomic").unwrap();
        assert_eq!(store.read_file("test.txt").unwrap(), b"atomic");
        assert!(!store.exists("test.txt.tmp"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

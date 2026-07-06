use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub trait StorageProvider: Send + Sync {
    fn write_file(&self, rel_path: &str, data: &[u8]) -> Result<()>;
    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>>;
    fn exists(&self, rel_path: &str) -> bool;
    fn list_files(&self, prefix: &str) -> Result<Vec<String>>;
    fn sync_up(&self) -> Result<()>;
    fn sync_down(&self) -> Result<()>;
    fn root(&self) -> &Path;
}

pub struct GitStorage {
    repo_path: PathBuf,
}

impl GitStorage {
    pub fn new(repo_path: PathBuf) -> Self {
        Self { repo_path }
    }

    pub fn lock(&self) -> Result<std::fs::File> {
        self.lock_inner(true)
    }

    pub fn try_lock(&self) -> Result<std::fs::File> {
        self.lock_inner(false)
    }

    fn lock_inner(&self, blocking: bool) -> Result<std::fs::File> {
        let lock_path = self.repo_path.join(".clync.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)
            .context("could not create lock file")?;
        if blocking {
            file.lock_exclusive()
                .context("another clync process is running (locked)")?;
        } else {
            file.try_lock_exclusive()
                .context("sync in progress, try again shortly")?;
        }
        Ok(file)
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
        Ok(Self {
            repo_path: repo_path.to_path_buf(),
        })
    }

    pub fn clone_repo(url: &str, dest: &Path) -> Result<Self> {
        let status = std::process::Command::new("git")
            .args(["clone", url, &dest.to_string_lossy()])
            .status()
            .context("git clone failed")?;
        if !status.success() {
            bail!("git clone failed");
        }
        Ok(Self {
            repo_path: dest.to_path_buf(),
        })
    }

    pub fn has_remote(&self) -> bool {
        std::process::Command::new("git")
            .args(["remote"])
            .current_dir(&self.repo_path)
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    }

    pub fn get_remote_url(&self) -> Option<String> {
        std::process::Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(&self.repo_path)
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
            .current_dir(&self.repo_path)
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
                .current_dir(&self.repo_path)
                .status();
            break;
        }
        Ok(())
    }

    fn run_git(&self, args: &[&str]) -> Result<()> {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.repo_path)
            .status()
            .with_context(|| format!("git {} failed", args.join(" ")))?;
        if !status.success() {
            bail!("git {} exited with {}", args.join(" "), status);
        }
        Ok(())
    }

    pub fn setup_lfs(&self) -> Result<()> {
        let output = std::process::Command::new("git")
            .args(["lfs", "version"])
            .output();
        match output {
            Ok(o) if o.status.success() => {}
            _ => bail!(
                "git-lfs is not installed. Install it first:\n  \
                 brew install git-lfs   # macOS\n  \
                 apt install git-lfs    # Debian/Ubuntu"
            ),
        }

        self.run_git(&["lfs", "install", "--local"])?;

        let attr_path = self.repo_path.join(".gitattributes");
        let pattern = "sessions/** filter=lfs diff=lfs merge=lfs -text";
        if attr_path.exists() {
            let content = std::fs::read_to_string(&attr_path)?;
            if content.contains(pattern) {
                return Ok(());
            }
            let mut file = std::fs::OpenOptions::new().append(true).open(&attr_path)?;
            std::io::Write::write_all(&mut file, format!("\n{pattern}\n").as_bytes())?;
        } else {
            std::fs::write(&attr_path, format!("{pattern}\n"))?;
        }

        Ok(())
    }

    pub fn commit(&self, message: &str) -> Result<()> {
        self.run_git(&["add", "-A"])?;

        let status = std::process::Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(&self.repo_path)
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
                .current_dir(&self.repo_path)
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
            .current_dir(&self.repo_path)
            .output()
            .context("git pull failed")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Not possible to fast-forward") || stderr.contains("divergent") {
                bail!(
                    "git pull failed: repos have diverged. \
                     Try: cd {} && git pull --rebase",
                    self.repo_path.display()
                );
            }
            bail!("git pull --ff-only failed: {}", stderr.trim());
        }
        Ok(())
    }
}

impl StorageProvider for GitStorage {
    fn write_file(&self, rel_path: &str, data: &[u8]) -> Result<()> {
        let full_path = self.repo_path.join(rel_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full_path, data)?;
        Ok(())
    }

    fn read_file(&self, rel_path: &str) -> Result<Vec<u8>> {
        let full_path = self.repo_path.join(rel_path);
        std::fs::read(&full_path).with_context(|| format!("could not read {}", full_path.display()))
    }

    fn exists(&self, rel_path: &str) -> bool {
        self.repo_path.join(rel_path).exists()
    }

    fn list_files(&self, prefix: &str) -> Result<Vec<String>> {
        let dir = self.repo_path.join(prefix);
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

    fn sync_up(&self) -> Result<()> {
        self.commit("clync sync")?;
        self.push_remote()
    }

    fn sync_down(&self) -> Result<()> {
        self.pull_remote()
    }

    fn root(&self) -> &Path {
        &self.repo_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("clync-storage-test")
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
        let storage = GitStorage::init_repo(&repo_path).unwrap();
        assert!(repo_path.join(".git").exists());
        assert!(repo_path.join(".gitignore").exists());
        assert_eq!(storage.root(), repo_path);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn init_repo_idempotent() {
        let dir = temp_dir("init_idem");
        let repo_path = dir.join("repo");
        GitStorage::init_repo(&repo_path).unwrap();
        GitStorage::init_repo(&repo_path).unwrap();
        assert!(repo_path.join(".git").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_read_exists() {
        let dir = temp_dir("write_read");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        storage.write_file("test.txt", b"hello").unwrap();
        assert!(storage.exists("test.txt"));
        assert!(!storage.exists("missing.txt"));
        let data = storage.read_file("test.txt").unwrap();
        assert_eq!(data, b"hello");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn write_creates_subdirs() {
        let dir = temp_dir("subdirs");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        storage.write_file("a/b/c.txt", b"nested").unwrap();
        assert!(storage.exists("a/b/c.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_files_works() {
        let dir = temp_dir("list_files");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        storage.write_file("sessions/a.txt", b"a").unwrap();
        storage.write_file("sessions/b.txt", b"b").unwrap();
        let files = storage.list_files("sessions").unwrap();
        assert_eq!(files.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_files_empty() {
        let dir = temp_dir("list_empty");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        let files = storage.list_files("nonexistent").unwrap();
        assert!(files.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn has_remote_false_for_new_repo() {
        let dir = temp_dir("no_remote");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        assert!(!storage.has_remote());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_remote_url_none_for_new_repo() {
        let dir = temp_dir("no_remote_url");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        assert!(storage.get_remote_url().is_none());
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

        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        storage.add_remote(bare.to_str().unwrap()).unwrap();
        assert!(storage.has_remote());
        assert!(storage.get_remote_url().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lock_and_try_lock() {
        let dir = temp_dir("lock");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        let _lock = storage.lock().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    fn set_git_test_author(repo: &Path) {
        std::process::Command::new("git")
            .args(["config", "user.email", "test@clync"])
            .current_dir(repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "test"])
            .current_dir(repo)
            .output()
            .unwrap();
    }

    #[test]
    fn commit_with_changes() {
        let dir = temp_dir("commit");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        set_git_test_author(storage.root());
        storage.write_file("file.txt", b"content").unwrap();
        storage.commit("test commit").unwrap();

        let log = std::process::Command::new("git")
            .args(["log", "--oneline"])
            .current_dir(storage.root())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&log.stdout);
        assert!(stdout.contains("test commit"), "log: {stdout}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_no_changes_is_noop() {
        let dir = temp_dir("commit_noop");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        set_git_test_author(storage.root());
        storage.write_file("file.txt", b"content").unwrap();
        storage.commit("first").unwrap();
        storage.commit("second should be noop").unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pull_remote_no_remote_is_ok() {
        let dir = temp_dir("pull_no_remote");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        storage.pull_remote().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn push_remote_no_remote_is_ok() {
        let dir = temp_dir("push_no_remote");
        let storage = GitStorage::init_repo(&dir.join("repo")).unwrap();
        storage.push_remote().unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}

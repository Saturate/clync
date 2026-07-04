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

    #[allow(dead_code)]
    pub fn init(repo_path: &Path) -> Result<Self> {
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
            std::fs::write(repo_path.join(".gitignore"), "# clync sync repo\n")?;
        }
        Ok(Self {
            repo_path: repo_path.to_path_buf(),
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

    #[allow(dead_code)]
    pub fn add_remote(&self, url: &str) -> Result<()> {
        self.run_git(&["remote", "add", "origin", url])
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

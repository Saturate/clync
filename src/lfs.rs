use anyhow::{Context, Result, bail};
use std::path::Path;

pub fn ensure_lfs_for_file(repo_path: &Path, rel_path: &str) -> Result<()> {
    check_lfs_installed()?;
    install_lfs_local(repo_path)?;

    let attr_path = repo_path.join(".gitattributes");
    let pattern = format!("{rel_path} filter=lfs diff=lfs merge=lfs -text");

    if attr_path.exists() {
        let content = std::fs::read_to_string(&attr_path)?;
        if content.contains(&pattern) {
            return Ok(());
        }
        let mut file = std::fs::OpenOptions::new().append(true).open(&attr_path)?;
        std::io::Write::write_all(&mut file, format!("{pattern}\n").as_bytes())?;
    } else {
        std::fs::write(&attr_path, format!("{pattern}\n"))?;
    }

    Ok(())
}

fn check_lfs_installed() -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["lfs", "version"])
        .output();
    match output {
        Ok(o) if o.status.success() => Ok(()),
        _ => bail!(
            "git-lfs is not installed. Install it first:\n  \
             brew install git-lfs   # macOS\n  \
             apt install git-lfs    # Debian/Ubuntu"
        ),
    }
}

fn install_lfs_local(repo_path: &Path) -> Result<()> {
    let marker = repo_path.join(".git").join("lfs");
    if marker.exists() {
        return Ok(());
    }
    let status = std::process::Command::new("git")
        .args(["lfs", "install", "--local"])
        .current_dir(repo_path)
        .status()
        .context("git lfs install failed")?;
    if !status.success() {
        bail!("git lfs install --local failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn lfs_available() -> bool {
        std::process::Command::new("git")
            .args(["lfs", "version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn temp_git_repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("clync-lfs-test")
            .join(name)
            .join(format!("{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).ok();
        }
        std::fs::create_dir_all(&dir).unwrap();
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&dir)
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn ensure_lfs_creates_gitattributes() {
        if !lfs_available() {
            return;
        }
        let repo = temp_git_repo("creates_attr");
        ensure_lfs_for_file(&repo, "sessions/big.jsonl.age").unwrap();

        let attr = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert!(attr.contains("sessions/big.jsonl.age filter=lfs diff=lfs merge=lfs -text"));

        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn ensure_lfs_idempotent() {
        if !lfs_available() {
            return;
        }
        let repo = temp_git_repo("idempotent");
        ensure_lfs_for_file(&repo, "sessions/a.age").unwrap();
        ensure_lfs_for_file(&repo, "sessions/a.age").unwrap();

        let attr = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert_eq!(
            attr.matches("sessions/a.age filter=lfs").count(),
            1,
            "pattern should appear exactly once"
        );

        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn ensure_lfs_appends_to_existing_gitattributes() {
        if !lfs_available() {
            return;
        }
        let repo = temp_git_repo("appends");
        std::fs::write(repo.join(".gitattributes"), "*.bin filter=lfs\n").unwrap();

        ensure_lfs_for_file(&repo, "sessions/new.age").unwrap();

        let attr = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert!(attr.starts_with("*.bin filter=lfs\n"));
        assert!(attr.contains("sessions/new.age filter=lfs diff=lfs merge=lfs -text"));

        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn ensure_lfs_multiple_files() {
        if !lfs_available() {
            return;
        }
        let repo = temp_git_repo("multi");
        ensure_lfs_for_file(&repo, "sessions/a.age").unwrap();
        ensure_lfs_for_file(&repo, "sessions/b.age").unwrap();

        let attr = std::fs::read_to_string(repo.join(".gitattributes")).unwrap();
        assert!(attr.contains("sessions/a.age"));
        assert!(attr.contains("sessions/b.age"));

        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn check_lfs_installed_returns_ok_when_available() {
        if !lfs_available() {
            return;
        }
        assert!(check_lfs_installed().is_ok());
    }

    #[test]
    fn install_lfs_local_skips_when_marker_exists() {
        if !lfs_available() {
            return;
        }
        let repo = temp_git_repo("marker");
        let marker = repo.join(".git").join("lfs");
        std::fs::create_dir_all(&marker).unwrap();
        install_lfs_local(&repo).unwrap();
    }
}

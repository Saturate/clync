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

/// Migrate files listed in `.gitattributes` from regular blobs to LFS
/// pointers across git history. Scopes to unpushed commits when an
/// upstream tracking ref exists so the push stays a fast-forward.
pub fn migrate_lfs_files(repo_path: &Path) -> Result<()> {
    check_lfs_installed()?;
    install_lfs_local(repo_path)?;

    let attr_path = repo_path.join(".gitattributes");
    if !attr_path.exists() {
        bail!("no .gitattributes found; set up LFS tracking first");
    }

    let content = std::fs::read_to_string(&attr_path)?;
    let patterns: Vec<&str> = content
        .lines()
        .filter_map(|line| {
            if line.contains("filter=lfs") {
                line.split_whitespace().next()
            } else {
                None
            }
        })
        .collect();

    if patterns.is_empty() {
        bail!("no LFS-tracked patterns in .gitattributes");
    }

    let include = patterns.join(",");
    let mut args = vec!["lfs", "migrate", "import", "--include", &include];

    let upstream = upstream_tracking_ref(repo_path);
    let include_ref;
    let exclude_ref;
    if let Some(ref u) = upstream {
        include_ref = "HEAD".to_string();
        exclude_ref = u.clone();
        args.extend(["--include-ref", &include_ref, "--exclude-ref", &exclude_ref]);
    }

    let output = std::process::Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .context("git lfs migrate import failed")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git lfs migrate import failed: {}", stderr.trim());
    }

    Ok(())
}

fn upstream_tracking_ref(repo_path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "@{upstream}"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if output.status.success() {
        let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !name.is_empty() {
            return Some(format!("refs/remotes/{name}"));
        }
    }
    None
}

pub fn is_large_file_push_error(stderr: &str) -> bool {
    (stderr.contains("exceeds") && stderr.contains("file size")) || stderr.contains("GH001")
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

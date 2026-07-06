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

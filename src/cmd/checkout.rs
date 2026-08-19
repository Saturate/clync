use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::crypto::Cipher;
use crate::manifest::Manifest;
use crate::resolver::{
    build_remote_map, decode_project_dir, normalize_remote, resolve_project_dir,
};
use crate::store::{Store, create_store};

pub struct UnmappedProject {
    pub remote_url: String,
    pub normalized_remote: String,
    pub session_count: usize,
    pub suggested_clone_path: PathBuf,
}

pub struct UnmappedResult {
    pub projects: Vec<UnmappedProject>,
    pub no_remote_projects: usize,
    pub no_remote_sessions: usize,
}

pub struct CloneAction {
    pub remote_url: String,
    pub clone_path: PathBuf,
}

pub fn cmd_checkout(
    list: bool,
    all: bool,
    base: Option<PathBuf>,
    path: Option<PathBuf>,
    remote: Option<String>,
) -> Result<()> {
    let config = Config::load()?;
    let cipher = Cipher::from_config(&config.encryption)?;
    let store = create_store(&config)?;

    let base_override = base.or(config.sync.clone_base.clone());
    let result = find_unmapped_projects(&config, &cipher, store.as_ref(), &base_override)?;
    let unmapped = result.projects;

    if unmapped.is_empty() {
        println!("all projects with remote URLs are cloned locally.");
        if result.no_remote_projects > 0 {
            println!(
                "note: {} project{} ({} session{}) have no git remote and can't be auto-cloned.",
                result.no_remote_projects,
                if result.no_remote_projects == 1 {
                    ""
                } else {
                    "s"
                },
                result.no_remote_sessions,
                if result.no_remote_sessions == 1 {
                    ""
                } else {
                    "s"
                },
            );
        }
        return Ok(());
    }

    if list {
        print_unmapped(&unmapped);
        if result.no_remote_projects > 0 {
            println!(
                "note: {} project{} ({} session{}) have no git remote and can't be auto-cloned.",
                result.no_remote_projects,
                if result.no_remote_projects == 1 {
                    ""
                } else {
                    "s"
                },
                result.no_remote_sessions,
                if result.no_remote_sessions == 1 {
                    ""
                } else {
                    "s"
                },
            );
        }
        return Ok(());
    }

    if let Some(ref target_remote) = remote {
        let normalized = normalize_remote(target_remote);
        let project = unmapped
            .iter()
            .find(|p| p.normalized_remote == normalized || p.remote_url == *target_remote);
        match project {
            Some(p) => {
                let clone_path = path.unwrap_or_else(|| p.suggested_clone_path.clone());
                clone_repo(&p.remote_url, &clone_path)?;
                println!("cloned {} to {}", p.remote_url, clone_path.display());
            }
            None => bail!("no unmapped project matching '{target_remote}'"),
        }
        return Ok(());
    }

    if all {
        let mut cloned = 0u32;
        let mut failed = 0u32;
        for p in &unmapped {
            let clone_path = p.suggested_clone_path.clone();
            match clone_repo(&p.remote_url, &clone_path) {
                Ok(()) => {
                    println!("  cloned {} to {}", p.remote_url, clone_path.display());
                    cloned += 1;
                }
                Err(e) => {
                    eprintln!("  failed {}: {e}", p.remote_url);
                    failed += 1;
                }
            }
        }
        println!("{cloned} cloned, {failed} failed");
        if result.no_remote_projects > 0 {
            println!(
                "note: {} project{} ({} session{}) have no git remote and can't be auto-cloned.",
                result.no_remote_projects,
                if result.no_remote_projects == 1 {
                    ""
                } else {
                    "s"
                },
                result.no_remote_sessions,
                if result.no_remote_sessions == 1 {
                    ""
                } else {
                    "s"
                },
            );
        }
        if failed > 0 {
            bail!("{failed} clone(s) failed");
        }
        return Ok(());
    }

    // Interactive TUI
    if !std::io::stdin().is_terminal() {
        bail!("not a terminal. use --list, --all, or specify a remote.");
    }

    let actions = crate::checkout_tui::run_tui(&unmapped)?;
    if actions.is_empty() {
        println!("nothing selected.");
        return Ok(());
    }

    let mut cloned = 0u32;
    let mut failed = 0u32;
    for action in &actions {
        match clone_repo(&action.remote_url, &action.clone_path) {
            Ok(()) => {
                println!(
                    "  cloned {} to {}",
                    action.remote_url,
                    action.clone_path.display()
                );
                cloned += 1;
            }
            Err(e) => {
                eprintln!("  failed {}: {e}", action.remote_url);
                failed += 1;
            }
        }
    }
    println!("{cloned} repos cloned. run `clync pull` to sync sessions.");
    if failed > 0 {
        bail!("{failed} clone(s) failed");
    }

    Ok(())
}

pub fn find_unmapped_projects(
    config: &Config,
    cipher: &Cipher,
    store: &dyn Store,
    base_override: &Option<PathBuf>,
) -> Result<UnmappedResult> {
    let manifest_rel = if matches!(config.encryption, crate::config::EncryptionConfig::None) {
        "manifest.json"
    } else {
        "manifest.json.age"
    };

    if !store.exists(manifest_rel) {
        return Ok(UnmappedResult {
            projects: Vec::new(),
            no_remote_projects: 0,
            no_remote_sessions: 0,
        });
    }

    let data = store.read_file(manifest_rel)?;
    let plain = cipher.decrypt(&data)?;
    let manifest: Manifest = serde_json::from_slice(&plain)?;

    let projects_dir = config.claude_projects_dir();
    let remote_map = build_remote_map(&projects_dir);

    let mut project_sessions: HashMap<String, (Option<String>, usize)> = HashMap::new();
    for entry in manifest.sessions.values() {
        project_sessions
            .entry(entry.project_path.clone())
            .and_modify(|(existing_url, count)| {
                *count += 1;
                if existing_url.is_none() {
                    *existing_url = entry.remote_url.clone();
                }
            })
            .or_insert((entry.remote_url.clone(), 1));
    }

    let mut no_remote_projects = 0usize;
    let mut no_remote_sessions = 0usize;
    let mut unmapped = Vec::new();

    for (project_path, (remote_url, session_count)) in &project_sessions {
        let remote_url = match remote_url {
            Some(url) => url,
            None => {
                no_remote_projects += 1;
                no_remote_sessions += session_count;
                continue;
            }
        };

        let resolved = resolve_project_dir(project_path, &remote_map, &projects_dir);
        let dir_name = resolved.unwrap_or_default();

        let real_path = decode_project_dir(&dir_name);
        if std::path::Path::new(&real_path).exists() {
            continue;
        }

        let normalized = normalize_remote(remote_url);
        let suggested = derive_clone_path(&normalized, base_override);

        unmapped.push(UnmappedProject {
            remote_url: remote_url.clone(),
            normalized_remote: normalized,
            session_count: *session_count,
            suggested_clone_path: suggested,
        });
    }

    unmapped.sort_by(|a, b| {
        a.normalized_remote
            .cmp(&b.normalized_remote)
            .then_with(|| b.session_count.cmp(&a.session_count))
    });
    dedup_by_remote(&mut unmapped);

    Ok(UnmappedResult {
        projects: unmapped,
        no_remote_projects,
        no_remote_sessions,
    })
}

fn dedup_by_remote(projects: &mut Vec<UnmappedProject>) {
    let mut seen = std::collections::HashSet::new();
    projects.retain(|p| seen.insert(p.normalized_remote.clone()));
}

pub fn derive_clone_path(normalized_remote: &str, clone_base: &Option<PathBuf>) -> PathBuf {
    match clone_base {
        Some(base) => base.join(repo_name_from_remote(normalized_remote)),
        None => {
            let home = crate::config::home_dir().unwrap_or_else(|| PathBuf::from("."));
            home.join("code").join(normalized_remote)
        }
    }
}

fn repo_name_from_remote(normalized_remote: &str) -> &str {
    normalized_remote
        .rsplit('/')
        .next()
        .unwrap_or(normalized_remote)
}

fn clone_repo(url: &str, target: &Path) -> Result<()> {
    if target.exists() {
        bail!("target directory already exists: {}", target.display());
    }
    let output = std::process::Command::new("git")
        .arg("clone")
        .arg("--quiet")
        .arg(url)
        .arg(target)
        .output()
        .context("git clone failed")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone failed for {url}: {}", stderr.trim());
    }
    Ok(())
}

fn print_unmapped(projects: &[UnmappedProject]) {
    println!(
        "{} unmapped project{}:",
        projects.len(),
        if projects.len() == 1 { "" } else { "s" }
    );
    println!();
    for p in projects {
        println!(
            "  {} ({} session{})",
            p.remote_url,
            p.session_count,
            if p.session_count == 1 { "" } else { "s" }
        );
        println!("    -> {}", p.suggested_clone_path.display());
    }
    println!();
    println!("run `clync checkout` to select and clone, or `clync checkout --all` to clone all.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_clone_path_with_base() {
        let path = derive_clone_path("github.com/user/repo", &Some(PathBuf::from("/code")));
        assert_eq!(path, PathBuf::from("/code/repo"));
    }

    #[test]
    fn derive_clone_path_without_base() {
        let path = derive_clone_path("github.com/user/repo", &None);
        assert!(path.ends_with("code/github.com/user/repo"));
    }

    #[test]
    fn repo_name_from_remote_extracts_last_segment() {
        assert_eq!(repo_name_from_remote("github.com/user/repo"), "repo");
        assert_eq!(repo_name_from_remote("gitlab.com/group/sub/proj"), "proj");
        assert_eq!(repo_name_from_remote("solo"), "solo");
    }

    #[test]
    fn dedup_removes_duplicates() {
        let mut projects = vec![
            UnmappedProject {
                remote_url: "git@github.com:user/a.git".into(),
                normalized_remote: "github.com/user/a".into(),
                session_count: 1,
                suggested_clone_path: PathBuf::from("/tmp/a"),
            },
            UnmappedProject {
                remote_url: "https://github.com/user/a".into(),
                normalized_remote: "github.com/user/a".into(),
                session_count: 2,
                suggested_clone_path: PathBuf::from("/tmp/a2"),
            },
        ];
        dedup_by_remote(&mut projects);
        assert_eq!(projects.len(), 1);
    }
}

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

pub fn build_remote_map(claude_projects_dir: &Path) -> HashMap<String, String> {
    build_remote_map_with(claude_projects_dir, &git_remote_url)
}

pub fn build_remote_map_with(
    claude_projects_dir: &Path,
    get_remote: &dyn Fn(&str) -> Option<String>,
) -> HashMap<String, String> {
    let mut map = HashMap::new();

    let entries = match std::fs::read_dir(claude_projects_dir) {
        Ok(e) => e,
        Err(_) => return map,
    };

    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let real_path = decode_project_dir(&dir_name);
        if let Some(remote) = get_remote(&real_path) {
            let normalized = normalize_remote(&remote);
            map.insert(normalized, dir_name);
        }
    }

    map
}

pub fn resolve_project_dir(
    normalized_path: &str,
    remote_map: &HashMap<String, String>,
    claude_projects_dir: &Path,
) -> Option<String> {
    let candidate = crate::manifest::denormalize_project_path(normalized_path);
    let candidate_dir = claude_projects_dir.join(&candidate);
    if candidate_dir.exists() {
        return Some(candidate);
    }

    let candidate_real = decode_project_dir(&candidate);
    if let Some(remote) = git_remote_url(&candidate_real) {
        let normalized = normalize_remote(&remote);
        if let Some(local_dir) = remote_map.get(&normalized) {
            return Some(local_dir.clone());
        }
    }

    if let Ok(entries) = std::fs::read_dir(claude_projects_dir) {
        let suffix = format!("-{normalized_path}");
        let mut matches: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if dir_name.ends_with(&suffix) || dir_name == normalized_path {
                matches.push(dir_name);
            }
        }
        if matches.len() == 1 {
            return Some(matches.into_iter().next().unwrap());
        }
    }

    let decoded_remote_path = decode_project_dir(&format!("-placeholder-{}", normalized_path));
    let repo_name = Path::new(&decoded_remote_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string());

    if let Some(name) = repo_name {
        for local_dir in remote_map.values() {
            if local_dir.ends_with(&format!("-{name}")) {
                return Some(local_dir.clone());
            }
        }
    }

    Some(candidate)
}

pub fn decode_project_dir(encoded: &str) -> String {
    let path = encoded.replace('-', "/");
    format!("/{}", path.trim_start_matches('/'))
}

pub fn git_remote_url(path: &str) -> Option<String> {
    let path = Path::new(path);
    if !path.exists() {
        return None;
    }
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(path)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

pub fn normalize_remote(url: &str) -> String {
    let s = url.trim_end_matches('/').trim_end_matches(".git");

    let s = s
        .strip_prefix("ssh://")
        .or_else(|| s.strip_prefix("https://"))
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);

    if let Some(rest) = s.strip_prefix("git@") {
        return rest.replace(':', "/").to_lowercase();
    }

    s.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_project_dir_basic() {
        assert_eq!(
            decode_project_dir("-Users-akj-code-myapp"),
            "/Users/akj/code/myapp"
        );
    }

    #[test]
    fn decode_project_dir_leading_dash() {
        assert_eq!(decode_project_dir("code-myapp"), "/code/myapp");
    }

    #[test]
    fn normalize_remote_ssh() {
        assert_eq!(
            normalize_remote("git@github.com:User/Repo.git"),
            "github.com/user/repo"
        );
    }

    #[test]
    fn normalize_remote_https() {
        assert_eq!(
            normalize_remote("https://github.com/User/Repo.git"),
            "github.com/user/repo"
        );
    }

    #[test]
    fn normalize_remote_http() {
        assert_eq!(
            normalize_remote("http://gitlab.com/Group/Project/"),
            "gitlab.com/group/project"
        );
    }

    #[test]
    fn normalize_remote_ssh_prefix() {
        assert_eq!(
            normalize_remote("ssh://git@github.com/User/Repo.git"),
            "github.com/user/repo"
        );
    }

    #[test]
    fn normalize_ssh_variants_match() {
        let a = normalize_remote("git@github.com:User/Repo.git");
        let b = normalize_remote("ssh://git@github.com/User/Repo.git");
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_remote_plain() {
        assert_eq!(normalize_remote("example.com/repo"), "example.com/repo");
    }

    #[test]
    fn build_remote_map_with_mock() {
        let dir = std::env::temp_dir().join(format!("clync-resolver-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("project-alpha")).unwrap();
        std::fs::create_dir_all(dir.join("project-beta")).unwrap();

        let remotes: HashMap<String, String> = [(
            "/project/alpha".to_string(),
            "git@github.com:user/alpha.git".to_string(),
        )]
        .into_iter()
        .collect();

        let map = build_remote_map_with(&dir, &|path: &str| remotes.get(path).cloned());

        assert_eq!(map.len(), 1);
        assert!(map.contains_key("github.com/user/alpha"));
        assert_eq!(map.get("github.com/user/alpha").unwrap(), "project-alpha");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_remote_map_with_nonexistent_dir() {
        let map = build_remote_map_with(Path::new("/nonexistent"), &|_| None);
        assert!(map.is_empty());
    }

    #[test]
    fn resolve_project_dir_by_suffix() {
        let dir = std::env::temp_dir().join(format!("clync-resolve-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Users-someone-code-myproject")).unwrap();

        let map = HashMap::new();
        let result = resolve_project_dir("code-myproject", &map, &dir);
        assert_eq!(result, Some("Users-someone-code-myproject".to_string()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_project_dir_no_match_returns_denormalized() {
        let dir =
            std::env::temp_dir().join(format!("clync-resolve-nomatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let map = HashMap::new();
        let result = resolve_project_dir("some-project", &map, &dir);
        assert!(result.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_project_dir_ambiguous_suffix_falls_through() {
        let dir = std::env::temp_dir().join(format!("clync-resolve-ambig-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Users-alice-code-myproject")).unwrap();
        std::fs::create_dir_all(dir.join("Users-bob-code-myproject")).unwrap();

        let map = HashMap::new();
        let result = resolve_project_dir("code-myproject", &map, &dir);
        // Two suffix matches means it can't pick one, falls through to repo name / candidate
        assert!(result.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_project_dir_by_remote_map_repo_name() {
        let dir =
            std::env::temp_dir().join(format!("clync-resolve-reponame-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut map = HashMap::new();
        map.insert(
            "github.com/user/myrepo".to_string(),
            "Users-alice-code-myrepo".to_string(),
        );

        // The normalized_path "code-myrepo" decodes to repo name "myrepo",
        // which matches the remote_map value suffix "-myrepo"
        let result = resolve_project_dir("code-myrepo", &map, &dir);
        assert_eq!(result, Some("Users-alice-code-myrepo".to_string()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_remote_map_with_skips_files() {
        let dir = std::env::temp_dir().join(format!("clync-map-skipfiles-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("project-alpha")).unwrap();
        std::fs::write(dir.join("not-a-dir.txt"), "file").unwrap();

        let remotes: HashMap<String, String> = [
            (
                "/project/alpha".to_string(),
                "git@github.com:user/alpha.git".to_string(),
            ),
            (
                "/not/a/dir.txt".to_string(),
                "git@github.com:user/file.git".to_string(),
            ),
        ]
        .into_iter()
        .collect();

        let map = build_remote_map_with(&dir, &|path: &str| remotes.get(path).cloned());
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("github.com/user/alpha"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn normalize_remote_trailing_slashes_stripped() {
        // trim_end_matches strips all trailing slashes, then .git suffix
        assert_eq!(
            normalize_remote("https://github.com/User/Repo.git///"),
            "github.com/user/repo"
        );
        assert_eq!(
            normalize_remote("https://github.com/User/Repo/"),
            "github.com/user/repo"
        );
    }

    #[test]
    fn normalize_remote_mixed_case_preserved_in_path() {
        // Normalization lowercases everything
        assert_eq!(
            normalize_remote("https://GitHub.COM/MyOrg/MyRepo.git"),
            "github.com/myorg/myrepo"
        );
    }

    #[test]
    fn decode_project_dir_single_segment() {
        assert_eq!(decode_project_dir("project"), "/project");
    }

    #[test]
    fn git_remote_url_nonexistent_path() {
        assert_eq!(git_remote_url("/nonexistent/path/to/repo"), None);
    }
}

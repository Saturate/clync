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

    if let Some(rest) = s.strip_prefix("git@") {
        return rest.replace(':', "/").to_lowercase();
    }

    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .or_else(|| s.strip_prefix("ssh://"))
        .unwrap_or(s);

    s.to_lowercase()
}

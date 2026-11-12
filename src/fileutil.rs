use anyhow::Result;
use std::path::Path;

use crate::compress;
use crate::config::Config;
use crate::crypto::Cipher;
use crate::ignore::{IgnoreRules, strip_storage_suffix};

pub fn encrypted_name(name: &str, encrypted: bool) -> String {
    storage_name(name, encrypted, false)
}

pub fn storage_name(name: &str, encrypted: bool, compressed: bool) -> String {
    match (encrypted, compressed) {
        (true, true) => format!("{name}.zst.age"),
        (true, false) => format!("{name}.age"),
        (false, true) => format!("{name}.zst"),
        (false, false) => name.to_string(),
    }
}

pub fn is_encrypted(config: &Config) -> bool {
    !matches!(config.encryption, crate::config::EncryptionConfig::None)
}

pub fn is_traversal_safe(s: &str) -> bool {
    !s.contains('/') && !s.contains('\\') && !s.contains('\0') && s != ".."
}

pub fn is_safe_path_component(s: &str) -> bool {
    if s.is_empty() || !is_traversal_safe(s) {
        return false;
    }
    !s.split('-').any(|seg| seg == "..")
}

pub fn mtime_secs(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0))
}

pub fn sync_file_if_changed(
    src: &Path,
    dst: &Path,
    cipher: &Cipher,
    compressed: bool,
    compression_level: i32,
) -> Result<u32> {
    if !src.exists() {
        return Ok(0);
    }

    let src_mtime = mtime_secs(src)?;
    if dst.exists() && src_mtime <= mtime_secs(dst)? {
        return Ok(0);
    }

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let plaintext = std::fs::read(src)?;
    let payload = if compressed {
        compress::compress(&plaintext, compression_level)?
    } else {
        plaintext
    };
    let encrypted = cipher.encrypt(&payload)?;
    std::fs::write(dst, encrypted)?;
    Ok(1)
}

pub fn restore_file(src: &Path, dst: &Path, cipher: &Cipher) -> Result<u32> {
    if !src.exists() {
        return Ok(0);
    }

    if dst.exists() && mtime_secs(src)? <= mtime_secs(dst)? {
        return Ok(0);
    }

    let decrypted = match cipher.decrypt_file(src) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: could not decrypt {}: {e}", src.display());
            return Ok(0);
        }
    };
    let plaintext = compress::maybe_decompress(&decrypted)?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dst, plaintext)?;
    Ok(1)
}

pub fn sync_directory(
    src_dir: &Path,
    dst_dir: &Path,
    rules: &IgnoreRules,
    cipher: &Cipher,
    encrypted: bool,
    compressed: bool,
    compression_level: i32,
) -> Result<u32> {
    if !src_dir.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst_dir)?;
    rules.prune(dst_dir)?;
    let mut count = 0u32;
    for entry in rules.walk_files(src_dir, false) {
        let rel = entry.path().strip_prefix(src_dir)?;
        let dst = dst_dir.join(storage_name(&rel.to_string_lossy(), encrypted, compressed));
        count += sync_file_if_changed(entry.path(), &dst, cipher, compressed, compression_level)?;
    }
    Ok(count)
}

pub fn restore_directory(
    src_dir: &Path,
    dst_dir: &Path,
    rules: &IgnoreRules,
    cipher: &Cipher,
) -> Result<u32> {
    if !src_dir.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst_dir)?;
    let mut count = 0u32;
    // Skipping here too keeps junk an older version pushed off other machines
    for entry in rules.walk_files(src_dir, true) {
        let rel = entry
            .path()
            .strip_prefix(src_dir)?
            .to_string_lossy()
            .to_string();
        let dst = dst_dir.join(strip_storage_suffix(&rel));
        count += restore_file(entry.path(), &dst, cipher)?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use walkdir::WalkDir;

    fn tree(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("clync-ignore-test")
            .join(format!("{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        for (rel, body) in [
            ("my-skill/SKILL.md", "mine"),
            ("my-skill/node_modules/dep/index.js", "dep"),
            ("my-skill/scripts/__pycache__/x.pyc", "pyc"),
            ("my-skill/.git/HEAD", "ref"),
            ("synced/org_user/docx/SKILL.md", "managed"),
            (".trash/old/SKILL.md", "deleted"),
            ("nested/synced/SKILL.md", "only top-level synced is skipped"),
        ] {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        dir
    }

    fn files(root: &Path) -> Vec<String> {
        let mut out: Vec<String> = WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| {
                let rel = e.path().strip_prefix(root).unwrap();
                rel.to_string_lossy().replace('\\', "/")
            })
            .collect();
        out.sort();
        out
    }

    fn skip() -> IgnoreRules {
        IgnoreRules::new(&[], &["synced", ".trash"])
    }

    #[test]
    fn sync_directory_skips_ignored_dirs() {
        let src = tree("push-src");
        let dst = src.with_file_name(format!("push-dst-{}", std::process::id()));
        std::fs::remove_dir_all(&dst).ok();
        sync_directory(&src, &dst, &skip(), &Cipher::Plaintext, false, false, 3).unwrap();
        assert_eq!(files(&dst), ["my-skill/SKILL.md", "nested/synced/SKILL.md"]);
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn sync_directory_prunes_previously_pushed_junk() {
        // A store written by an older version that synced everything
        let dst = tree("prune-dst");
        let src = dst.with_file_name(format!("prune-src-{}", std::process::id()));
        std::fs::remove_dir_all(&src).ok();
        std::fs::create_dir_all(src.join("my-skill")).unwrap();
        std::fs::write(src.join("my-skill/SKILL.md"), "mine").unwrap();
        sync_directory(&src, &dst, &skip(), &Cipher::Plaintext, false, false, 3).unwrap();
        assert_eq!(files(&dst), ["my-skill/SKILL.md", "nested/synced/SKILL.md"]);
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn user_ignore_rules_apply_on_push_and_prune() {
        let src = tree("user-src");
        std::fs::write(src.join("my-skill/notes.tmp"), "scratch").unwrap();
        let dst = src.with_file_name(format!("user-dst-{}", std::process::id()));
        std::fs::remove_dir_all(&dst).ok();
        // First push with defaults, then tighten the rules: the stored
        // copies of newly ignored names must be pruned
        sync_directory(&src, &dst, &skip(), &Cipher::Plaintext, true, true, 3).unwrap();
        assert!(files(&dst).contains(&"my-skill/notes.tmp.zst.age".to_string()));
        let rules = IgnoreRules::new(
            &["notes.tmp".into(), "nested".into(), "!node_modules".into()],
            &["synced", ".trash"],
        );
        sync_directory(&src, &dst, &rules, &Cipher::Plaintext, true, true, 3).unwrap();
        assert_eq!(
            files(&dst),
            [
                "my-skill/SKILL.md.zst.age",
                "my-skill/node_modules/dep/index.js.zst.age",
            ]
        );
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn restore_directory_skips_ignored_dirs() {
        let src = tree("pull-src");
        let dst = src.with_file_name(format!("pull-dst-{}", std::process::id()));
        std::fs::remove_dir_all(&dst).ok();
        restore_directory(&src, &dst, &skip(), &Cipher::Plaintext).unwrap();
        assert_eq!(files(&dst), ["my-skill/SKILL.md", "nested/synced/SKILL.md"]);
        std::fs::remove_dir_all(&src).ok();
        std::fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn encrypted_name_adds_age_extension() {
        assert_eq!(encrypted_name("file.json", true), "file.json.age");
        assert_eq!(encrypted_name("file.json", false), "file.json");
    }

    #[test]
    fn safe_path_rejects_empty() {
        assert!(!is_safe_path_component(""));
    }

    #[test]
    fn safe_path_rejects_slashes() {
        assert!(!is_safe_path_component("a/b"));
        assert!(!is_safe_path_component("a\\b"));
    }

    #[test]
    fn safe_path_rejects_null_bytes() {
        assert!(!is_safe_path_component("a\0b"));
    }

    #[test]
    fn safe_path_rejects_traversal() {
        assert!(!is_safe_path_component(".."));
        assert!(!is_safe_path_component("foo-.."));
        assert!(!is_safe_path_component("..-bar"));
    }

    #[test]
    fn safe_path_accepts_valid_components() {
        assert!(is_safe_path_component("session-abc123"));
        assert!(is_safe_path_component("file.json"));
        assert!(is_safe_path_component("a-b-c"));
        assert!(is_safe_path_component(".hidden"));
    }

    #[test]
    fn traversal_safe_blocks_slashes_and_null() {
        assert!(!is_traversal_safe("a/b"));
        assert!(!is_traversal_safe("a\\b"));
        assert!(!is_traversal_safe("a\0b"));
        assert!(!is_traversal_safe(".."));
    }

    #[test]
    fn traversal_safe_allows_empty_and_dash_segments() {
        assert!(is_traversal_safe(""));
        assert!(is_traversal_safe("foo-.."));
        assert!(is_traversal_safe("..-bar"));
        assert!(is_traversal_safe("session-abc123"));
        assert!(is_traversal_safe("712f0419-ea8f-4ae7-8407-a3242ab69193"));
    }
}

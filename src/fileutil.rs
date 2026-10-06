use anyhow::Result;
use std::path::Path;
use walkdir::WalkDir;

use crate::config::Config;
use crate::crypto::Cipher;

pub fn encrypted_name(name: &str, encrypted: bool) -> String {
    storage_name(name, encrypted, false)
}

pub fn storage_name(name: &str, encrypted: bool, compressed: bool) -> String {
    match (encrypted, compressed) {
        (true, true) => format!("{name}.zst.age"),
        (true, false) => format!("{name}.age"),
        _ => name.to_string(),
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

pub fn sync_file_if_changed(src: &Path, dst: &Path, cipher: &Cipher) -> Result<u32> {
    if !src.exists() {
        return Ok(0);
    }

    let src_mtime = mtime_secs(src)?;
    if dst.exists() && src_mtime <= mtime_secs(dst)? {
        return Ok(0);
    }

    cipher.encrypt_file(src, dst)?;
    Ok(1)
}

pub fn restore_file(src: &Path, dst: &Path, cipher: &Cipher) -> Result<u32> {
    if !src.exists() {
        return Ok(0);
    }

    if dst.exists() && mtime_secs(src)? <= mtime_secs(dst)? {
        return Ok(0);
    }

    let plaintext = match cipher.decrypt_file(src) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: could not decrypt {}: {e}", src.display());
            return Ok(0);
        }
    };
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dst, plaintext)?;
    Ok(1)
}

pub fn sync_directory(
    src_dir: &Path,
    dst_dir: &Path,
    cipher: &Cipher,
    encrypted: bool,
) -> Result<u32> {
    if !src_dir.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst_dir)?;
    let mut count = 0u32;
    for entry in WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(src_dir)?;
        let dst = dst_dir.join(encrypted_name(&rel.to_string_lossy(), encrypted));
        count += sync_file_if_changed(entry.path(), &dst, cipher)?;
    }
    Ok(count)
}

pub fn restore_directory(src_dir: &Path, dst_dir: &Path, cipher: &Cipher) -> Result<u32> {
    if !src_dir.exists() {
        return Ok(0);
    }
    std::fs::create_dir_all(dst_dir)?;
    let mut count = 0u32;
    for entry in WalkDir::new(src_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(src_dir)?
            .to_string_lossy()
            .to_string();
        let original_name = rel
            .strip_suffix(".zst.age")
            .or_else(|| rel.strip_suffix(".age"))
            .unwrap_or(&rel);
        let dst = dst_dir.join(original_name);
        count += restore_file(entry.path(), &dst, cipher)?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

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

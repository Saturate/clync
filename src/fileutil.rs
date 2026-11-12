use anyhow::Result;
use std::path::Path;
use walkdir::WalkDir;

use crate::compress;
use crate::config::Config;
use crate::crypto::Cipher;

/// Write a file only the current user can read (the age key).
pub fn write_secret_file(path: &Path, content: &str) -> Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?
    };
    #[cfg(not(unix))]
    let mut file = {
        // Lock the file down while it's still empty so the key never sits
        // on disk with inherited permissions.
        let file = std::fs::File::create(path)?;
        #[cfg(windows)]
        if let Err(e) = restrict_to_current_user(path) {
            eprintln!(
                "warning: could not restrict permissions on {}: {e}",
                path.display()
            );
        }
        file
    };
    file.write_all(content.as_bytes())?;
    Ok(())
}

/// Replace the file's ACL with a single full-control entry for the current
/// user, dropping inherited entries (the Windows equivalent of chmod 600).
#[cfg(windows)]
fn restrict_to_current_user(path: &Path) -> Result<()> {
    use anyhow::{Context, bail};

    let sid = current_user_sid()?;
    let out = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("*{sid}:F")])
        .output()
        .context("could not run icacls")?;
    if !out.status.success() {
        bail!(
            "icacls failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(windows)]
fn current_user_sid() -> Result<String> {
    use anyhow::Context;

    // `whoami /user /fo csv /nh` prints `"DOMAIN\user","S-1-5-21-..."`
    let out = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .context("could not run whoami")?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .rsplit(',')
        .next()
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| s.starts_with("S-"))
        .context("could not determine current user SID")
}

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
    // ':' would let a name like `C:foo` resolve outside the target dir on Windows
    !s.contains('/') && !s.contains('\\') && !s.contains(':') && !s.contains('\0') && s != ".."
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
    cipher: &Cipher,
    encrypted: bool,
    compressed: bool,
    compression_level: i32,
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
        let dst = dst_dir.join(storage_name(&rel.to_string_lossy(), encrypted, compressed));
        count += sync_file_if_changed(entry.path(), &dst, cipher, compressed, compression_level)?;
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
            .or_else(|| rel.strip_suffix(".zst"))
            .unwrap_or(&rel);
        let dst = dst_dir.join(original_name);
        count += restore_file(entry.path(), &dst, cipher)?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret_test_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join("clync-secret-test")
            .join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("key.txt")
    }

    #[test]
    fn write_secret_file_writes_content() {
        let path = secret_test_path("content");
        write_secret_file(&path, "AGE-SECRET-KEY-1\n").unwrap();
        write_secret_file(&path, "AGE-SECRET-KEY-2\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "AGE-SECRET-KEY-2\n"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn write_secret_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = secret_test_path("perms");
        write_secret_file(&path, "secret\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[cfg(windows)]
    #[test]
    fn write_secret_file_is_owner_only() {
        let path = secret_test_path("acl");
        write_secret_file(&path, "secret\n").unwrap();
        // `icacls /save` writes the DACL as SDDL (UTF-16), which uses SIDs
        // and so doesn't depend on the OS language
        let dump = path.with_file_name("acl.txt");
        let ok = std::process::Command::new("icacls")
            .arg(&path)
            .arg("/save")
            .arg(&dump)
            .status()
            .unwrap();
        assert!(ok.success());
        let raw = std::fs::read(&dump).unwrap();
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let sddl = String::from_utf16_lossy(&units);
        let dacl = sddl.lines().nth(1).unwrap().trim();

        // "D:P" = protected: inheritance from the parent is cut off
        assert!(dacl.starts_with("D:P"), "DACL not protected: {dacl}");
        let user = current_user_sid().unwrap();
        let aces: Vec<&str> = dacl
            .split(['(', ')'])
            .filter(|a| a.starts_with("A;"))
            .collect();
        assert!(
            aces.iter().any(|a| a.ends_with(user.as_str())),
            "user missing: {dacl}"
        );
        for ace in aces {
            let fields: Vec<&str> = ace.split(';').collect();
            assert!(!fields[1].contains("ID"), "inherited ACE: {ace}");
            // An elevated session also gives SYSTEM and Administrators
            // explicit entries on new files. Like root, they can read any
            // file anyway. Nobody else (Users, Everyone, ...) may appear.
            let sid = fields[5];
            assert!(
                sid == user || sid == "SY" || sid == "BA",
                "unexpected ACE {ace} in {dacl}"
            );
        }
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
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

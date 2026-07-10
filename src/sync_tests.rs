use super::*;
use crate::config::{
    EncryptionConfig, StorageConfig, SyncConfig, SyncTargets, default_lfs_threshold,
};

fn test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join("clync-sync-test")
        .join(name)
        .join(format!("{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).ok();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_config(encrypted: bool) -> Config {
    Config {
        sync: SyncConfig {
            claude_dir: std::path::PathBuf::from("/tmp/fake-claude"),
            include_companion_dirs: false,
            clone_base: None,
            storage: StorageConfig::Git {
                path: std::path::PathBuf::from("/tmp/fake-store"),
                auto_push: false,
                lfs_threshold: default_lfs_threshold(),
            },
        },
        encryption: if encrypted {
            EncryptionConfig::KeyFile {
                path: std::path::PathBuf::from("/tmp/fake-key"),
            }
        } else {
            EncryptionConfig::None
        },
        targets: SyncTargets::default(),
    }
}

#[test]
fn session_filename_encrypted() {
    assert_eq!(session_filename("abc-123", true), "abc-123.jsonl.age");
}

#[test]
fn session_filename_unencrypted() {
    assert_eq!(session_filename("abc-123", false), "abc-123.jsonl");
}

#[test]
fn manifest_filename_encrypted() {
    let config = make_config(true);
    assert_eq!(manifest_filename(&config), "manifest.json.age");
}

#[test]
fn manifest_filename_unencrypted() {
    let config = make_config(false);
    assert_eq!(manifest_filename(&config), "manifest.json");
}

#[test]
fn is_encrypted_check() {
    assert!(is_encrypted(&make_config(true)));
    assert!(!is_encrypted(&make_config(false)));
}

#[test]
fn tar_untar_roundtrip() {
    let dir = test_dir("tar_roundtrip");
    let src = dir.join("source");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("file1.txt"), "hello").unwrap();
    std::fs::write(src.join("file2.txt"), "world").unwrap();

    let tar_data = tar_directory(&src).unwrap();
    assert!(!tar_data.is_empty());

    let dst = dir.join("restored").join("source");
    untar_directory(&tar_data, &dst).unwrap();

    assert_eq!(
        std::fs::read_to_string(dst.join("file1.txt")).unwrap(),
        "hello"
    );
    assert_eq!(
        std::fs::read_to_string(dst.join("file2.txt")).unwrap(),
        "world"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tar_untar_nested_dirs() {
    let dir = test_dir("tar_nested");
    let src = dir.join("nested");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("sub").join("deep.txt"), "deep content").unwrap();

    let tar_data = tar_directory(&src).unwrap();
    let dst = dir.join("out").join("nested");
    untar_directory(&tar_data, &dst).unwrap();

    assert_eq!(
        std::fs::read_to_string(dst.join("sub").join("deep.txt")).unwrap(),
        "deep content"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tar_untar_overwrites_existing() {
    let dir = test_dir("tar_overwrite");
    let src = dir.join("src_dir");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("file.txt"), "new content").unwrap();

    let tar_data = tar_directory(&src).unwrap();

    let dst = dir.join("dst").join("src_dir");
    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(dst.join("file.txt"), "old content").unwrap();

    untar_directory(&tar_data, &dst).unwrap();
    assert_eq!(
        std::fs::read_to_string(dst.join("file.txt")).unwrap(),
        "new content"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn push_session_writes_to_store() {
    let dir = test_dir("push_session_basic");
    let store_dir = dir.join("store");
    std::fs::create_dir_all(&store_dir).unwrap();
    let store = crate::store::folder::FolderStore::new(store_dir);

    let jsonl_path = dir.join("test-session.jsonl");
    std::fs::write(&jsonl_path, b"session content").unwrap();

    let session = LocalSession {
        uuid: "abc-123".into(),
        project_dir_name: "project-a".into(),
        jsonl_path,
        companion_dir: None,
        entry: crate::manifest::SessionEntry {
            uuid: "abc-123".into(),
            project_path: "project-a".into(),
            mtime: 0,
            size: 15,
            content_hash: 12345,
            has_companion: false,
            last_pushed_by: "test".into(),
            remote_url: None,
        },
    };

    let cipher = crate::crypto::Cipher::Plaintext;
    push_session(&session, &cipher, &store, false, false).unwrap();

    assert!(store.exists("sessions/abc-123.jsonl"));
    assert_eq!(
        store.read_file("sessions/abc-123.jsonl").unwrap(),
        b"session content"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn push_session_with_companion_dir() {
    let dir = test_dir("push_session_companion");
    let store_dir = dir.join("store");
    std::fs::create_dir_all(&store_dir).unwrap();
    let store = crate::store::folder::FolderStore::new(store_dir);

    let jsonl_path = dir.join("sess-456.jsonl");
    std::fs::write(&jsonl_path, b"session data").unwrap();

    let companion = dir.join("sess-456");
    std::fs::create_dir_all(&companion).unwrap();
    std::fs::write(companion.join("tool-output.txt"), b"tool result").unwrap();

    let session = LocalSession {
        uuid: "sess-456".into(),
        project_dir_name: "project-b".into(),
        jsonl_path,
        companion_dir: Some(companion),
        entry: crate::manifest::SessionEntry {
            uuid: "sess-456".into(),
            project_path: "project-b".into(),
            mtime: 0,
            size: 12,
            content_hash: 99999,
            has_companion: true,
            last_pushed_by: "test".into(),
            remote_url: None,
        },
    };

    let cipher = crate::crypto::Cipher::Plaintext;
    push_session(&session, &cipher, &store, false, true).unwrap();

    assert!(store.exists("sessions/sess-456.jsonl"));
    assert!(store.exists("sessions/sess-456.dir.tar.gz"));

    let tar_data = store.read_file("sessions/sess-456.dir.tar.gz").unwrap();
    let restore_dir = dir.join("restored");
    std::fs::create_dir_all(&restore_dir).unwrap();
    untar_directory(&tar_data, &restore_dir.join("sess-456")).unwrap();
    assert!(
        restore_dir
            .join("sess-456")
            .join("tool-output.txt")
            .exists()
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn push_session_encrypted_uses_age_extension() {
    let dir = test_dir("push_session_enc");
    let store_dir = dir.join("store");
    std::fs::create_dir_all(&store_dir).unwrap();
    let store = crate::store::folder::FolderStore::new(store_dir);

    let jsonl_path = dir.join("enc-uuid.jsonl");
    std::fs::write(&jsonl_path, b"data").unwrap();

    let session = LocalSession {
        uuid: "enc-uuid".into(),
        project_dir_name: "proj".into(),
        jsonl_path,
        companion_dir: None,
        entry: crate::manifest::SessionEntry {
            uuid: "enc-uuid".into(),
            project_path: "proj".into(),
            mtime: 0,
            size: 4,
            content_hash: 0,
            has_companion: false,
            last_pushed_by: "test".into(),
            remote_url: None,
        },
    };

    let cipher = crate::crypto::Cipher::Plaintext;
    push_session(&session, &cipher, &store, true, false).unwrap();
    assert!(store.exists("sessions/enc-uuid.jsonl.age"));

    std::fs::remove_dir_all(&dir).ok();
}

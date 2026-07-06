use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::config::Config;
use crate::crypto::Cipher;
use crate::fileutil::is_safe_path_component;
use crate::manifest::Manifest;
use crate::merge::smart_merge;
use crate::parser::{entries_to_jsonl, parse_jsonl, parse_jsonl_file};
use crate::resolver::{build_remote_map, resolve_project_dir};
use crate::scanner::{LocalSession, ScanFilter, scan_sessions};
use crate::storage::StorageProvider;

fn session_filename(uuid: &str, encrypted: bool) -> String {
    if encrypted {
        format!("{uuid}.jsonl.age")
    } else {
        format!("{uuid}.jsonl")
    }
}

fn is_encrypted(config: &Config) -> bool {
    !matches!(config.encryption, crate::config::EncryptionConfig::None)
}

pub fn push(
    config: &Config,
    cipher: &Cipher,
    filter: &ScanFilter,
    storage: &dyn StorageProvider,
) -> Result<PushResult> {
    let manifest_rel = manifest_filename(config);
    let mut manifest = if storage.exists(&manifest_rel) {
        let data = storage.read_file(&manifest_rel)?;
        let plain = cipher.decrypt(&data)?;
        serde_json::from_slice(&plain)?
    } else {
        Manifest::new()
    };

    let local_sessions = scan_sessions(&config.claude_projects_dir(), filter)?;
    let encrypted = is_encrypted(config);
    let include_companions = config.sync.include_companion_dirs;

    let to_push: Vec<&LocalSession> = local_sessions
        .iter()
        .filter(|s| {
            manifest
                .sessions
                .get(&s.uuid)
                .is_none_or(|existing| existing.content_hash != s.entry.content_hash)
        })
        .collect();

    let skipped = (local_sessions.len() - to_push.len()) as u32;

    let results: Vec<_> = to_push
        .par_iter()
        .map(|session| {
            let result = push_session(session, cipher, storage, encrypted, include_companions);
            (session.uuid.clone(), session.entry.clone(), result)
        })
        .collect();

    let mut push_count = 0u32;
    for (uuid, entry, result) in results {
        match result {
            Ok(()) => {
                manifest.sessions.insert(uuid, entry);
                push_count += 1;
            }
            Err(e) => eprintln!("warning: {uuid}: {e}"),
        }
    }

    let lfs_threshold = config.sync.git.lfs_threshold;
    if lfs_threshold > 0 && push_count > 0 {
        let root = storage.root();
        for (uuid, _) in manifest.sessions.iter() {
            let filename = session_filename(uuid, encrypted);
            let path = root.join("sessions").join(&filename);
            if let Ok(meta) = std::fs::metadata(&path)
                && meta.len() >= lfs_threshold
            {
                let rel = format!("sessions/{filename}");
                match crate::lfs::ensure_lfs_for_file(root, &rel) {
                    Ok(()) => eprintln!(
                        "lfs: tracking {rel} ({:.1} MB)",
                        meta.len() as f64 / (1024.0 * 1024.0)
                    ),
                    Err(e) => eprintln!("warning: could not enable lfs for {rel}: {e}"),
                }
            }
        }
    }

    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let manifest_data = cipher.encrypt(manifest_json.as_bytes())?;
    let tmp_manifest = format!("{manifest_rel}.tmp");
    storage.write_file(&tmp_manifest, &manifest_data)?;
    let root = storage.root();
    std::fs::rename(root.join(&tmp_manifest), root.join(&manifest_rel))?;

    Ok(PushResult {
        pushed: push_count,
        skipped,
    })
}

fn push_session(
    session: &LocalSession,
    cipher: &Cipher,
    storage: &dyn StorageProvider,
    encrypted: bool,
    include_companions: bool,
) -> Result<()> {
    let filename = session_filename(&session.uuid, encrypted);
    let rel_path = format!("sessions/{filename}");
    let plaintext = std::fs::read(&session.jsonl_path)
        .with_context(|| format!("reading session {}", session.uuid))?;
    let data = cipher
        .encrypt(&plaintext)
        .with_context(|| format!("encrypting session {}", session.uuid))?;
    storage.write_file(&rel_path, &data)?;

    if include_companions && let Some(ref companion) = session.companion_dir {
        let tar_data = tar_directory(companion)?;
        let data = cipher.encrypt(&tar_data)?;
        let ext = if encrypted { "tar.gz.age" } else { "tar.gz" };
        storage.write_file(&format!("sessions/{}.dir.{ext}", session.uuid), &data)?;
    }

    Ok(())
}

enum PullAction {
    New {
        uuid: String,
        project_dir_name: String,
        rel_path: String,
    },
    Merge {
        uuid: String,
        project_dir_name: String,
        rel_path: String,
        local_jsonl_path: std::path::PathBuf,
    },
}

pub fn pull(
    config: &Config,
    cipher: &Cipher,
    filter: &ScanFilter,
    storage: &dyn StorageProvider,
) -> Result<PullResult> {
    let manifest_rel = manifest_filename(config);
    if !storage.exists(&manifest_rel) {
        return Ok(PullResult {
            pulled: 0,
            merged: 0,
            skipped: 0,
        });
    }

    let manifest_data = storage.read_file(&manifest_rel)?;
    let manifest_plain = cipher.decrypt(&manifest_data)?;
    let remote_manifest: Manifest = serde_json::from_slice(&manifest_plain)?;

    let projects_dir = config.claude_projects_dir();
    let local_sessions = scan_sessions(&projects_dir, filter)?;
    let local_map: std::collections::HashMap<String, &LocalSession> =
        local_sessions.iter().map(|s| (s.uuid.clone(), s)).collect();

    let remote_map = build_remote_map(&projects_dir);
    let encrypted = is_encrypted(config);

    let mut actions: Vec<PullAction> = Vec::new();
    let mut skipped = 0u32;

    for (uuid, remote_entry) in &remote_manifest.sessions {
        if !is_safe_path_component(uuid) || !is_safe_path_component(&remote_entry.project_path) {
            eprintln!("warning: skipping session with unsafe path: {uuid}");
            skipped += 1;
            continue;
        }

        let filename = session_filename(uuid, encrypted);
        let rel_path = format!("sessions/{filename}");

        if !storage.exists(&rel_path) {
            eprintln!("warning: session {uuid} not found in repo, skipping");
            skipped += 1;
            continue;
        }

        let project_dir_name =
            resolve_project_dir(&remote_entry.project_path, &remote_map, &projects_dir)
                .unwrap_or_else(|| {
                    crate::manifest::denormalize_project_path(&remote_entry.project_path)
                });

        if let Some(local) = local_map.get(uuid) {
            if local.entry.content_hash == remote_entry.content_hash {
                skipped += 1;
                continue;
            }
            actions.push(PullAction::Merge {
                uuid: uuid.clone(),
                project_dir_name,
                rel_path,
                local_jsonl_path: local.jsonl_path.clone(),
            });
        } else {
            actions.push(PullAction::New {
                uuid: uuid.clone(),
                project_dir_name,
                rel_path,
            });
        }
    }

    let pulled = AtomicU32::new(0);
    let merged = AtomicU32::new(0);

    let errors: Vec<_> = actions
        .par_iter()
        .filter_map(|action| {
            let result = match action {
                PullAction::New {
                    uuid,
                    project_dir_name,
                    rel_path,
                } => {
                    let r = pull_new(
                        uuid,
                        project_dir_name,
                        rel_path,
                        cipher,
                        storage,
                        &projects_dir,
                        encrypted,
                    );
                    if r.is_ok() {
                        pulled.fetch_add(1, Ordering::Relaxed);
                    }
                    r
                }
                PullAction::Merge {
                    uuid,
                    project_dir_name,
                    rel_path,
                    local_jsonl_path,
                } => {
                    let r = pull_merge(
                        uuid,
                        project_dir_name,
                        rel_path,
                        local_jsonl_path,
                        cipher,
                        storage,
                        &projects_dir,
                    );
                    if r.is_ok() {
                        merged.fetch_add(1, Ordering::Relaxed);
                    }
                    r
                }
            };
            let action_desc = match action {
                PullAction::New { uuid, rel_path, .. } => format!("{uuid} ({rel_path})"),
                PullAction::Merge { uuid, rel_path, .. } => format!("{uuid} merge ({rel_path})"),
            };
            match result {
                Ok(()) => None,
                Err(e) => Some(format!("{action_desc}: {e}")),
            }
        })
        .collect();

    for err in &errors {
        eprintln!("warning: {err}");
    }

    Ok(PullResult {
        pulled: pulled.load(Ordering::Relaxed),
        merged: merged.load(Ordering::Relaxed),
        skipped,
    })
}

fn pull_new(
    uuid: &str,
    project_dir_name: &str,
    rel_path: &str,
    cipher: &Cipher,
    storage: &dyn StorageProvider,
    projects_dir: &Path,
    encrypted: bool,
) -> Result<()> {
    let data = storage.read_file(rel_path)?;
    let plaintext = cipher.decrypt(&data)?;
    let target_dir = projects_dir.join(project_dir_name);
    std::fs::create_dir_all(&target_dir)?;
    std::fs::write(target_dir.join(format!("{uuid}.jsonl")), plaintext)?;

    let ext = if encrypted { "tar.gz.age" } else { "tar.gz" };
    let companion_rel = format!("sessions/{uuid}.dir.{ext}");
    if storage.exists(&companion_rel) {
        let tar_data = storage.read_file(&companion_rel)?;
        let plain_tar = cipher.decrypt(&tar_data)?;
        let companion_dir = target_dir.join(uuid);
        untar_directory(&plain_tar, &companion_dir)?;
    }

    Ok(())
}

fn pull_merge(
    uuid: &str,
    project_dir_name: &str,
    rel_path: &str,
    local_jsonl_path: &Path,
    cipher: &Cipher,
    storage: &dyn StorageProvider,
    projects_dir: &Path,
) -> Result<()> {
    let remote_data = storage.read_file(rel_path)?;
    let remote_plain = cipher.decrypt(&remote_data)?;
    let remote_entries = parse_jsonl(&remote_plain)?;
    let local_entries = parse_jsonl_file(local_jsonl_path)?;

    let merge_result = smart_merge(&local_entries, &remote_entries);
    let merged_data = entries_to_jsonl(&merge_result.entries)?;

    let target_dir = projects_dir.join(project_dir_name);
    std::fs::create_dir_all(&target_dir)?;
    std::fs::write(target_dir.join(format!("{uuid}.jsonl")), merged_data)?;
    Ok(())
}

pub fn status(
    config: &Config,
    cipher: &Cipher,
    filter: &ScanFilter,
    storage: &dyn StorageProvider,
) -> Result<StatusResult> {
    let manifest_rel = manifest_filename(config);
    let remote_manifest = if storage.exists(&manifest_rel) {
        let data = storage.read_file(&manifest_rel)?;
        let plain = cipher.decrypt(&data)?;
        serde_json::from_slice(&plain)?
    } else {
        Manifest::new()
    };

    let local_sessions = scan_sessions(&config.claude_projects_dir(), filter)?;
    let local_map: std::collections::HashMap<&str, &LocalSession> = local_sessions
        .iter()
        .map(|s| (s.uuid.as_str(), s))
        .collect();

    let mut local_only = Vec::new();
    let mut remote_only = Vec::new();
    let mut diverged = Vec::new();
    let mut in_sync = 0u32;

    for session in &local_sessions {
        match remote_manifest.sessions.get(&session.uuid) {
            None => local_only.push(SessionInfo {
                uuid: session.uuid.clone(),
                project: session.entry.project_path.clone(),
                size: session.entry.size,
            }),
            Some(remote) => {
                if session.entry.content_hash == remote.content_hash {
                    in_sync += 1;
                } else {
                    diverged.push(SessionInfo {
                        uuid: session.uuid.clone(),
                        project: session.entry.project_path.clone(),
                        size: session.entry.size,
                    });
                }
            }
        }
    }

    for (uuid, entry) in &remote_manifest.sessions {
        if !local_map.contains_key(uuid.as_str()) {
            remote_only.push(SessionInfo {
                uuid: uuid.clone(),
                project: entry.project_path.clone(),
                size: entry.size,
            });
        }
    }

    Ok(StatusResult {
        local_only,
        remote_only,
        diverged,
        in_sync,
    })
}

fn manifest_filename(config: &Config) -> String {
    if is_encrypted(config) {
        "manifest.json.age".into()
    } else {
        "manifest.json".into()
    }
}

pub struct PushResult {
    pub pushed: u32,
    pub skipped: u32,
}

pub struct PullResult {
    pub pulled: u32,
    pub merged: u32,
    pub skipped: u32,
}

pub struct SessionInfo {
    pub uuid: String,
    pub project: String,
    pub size: u64,
}

pub struct StatusResult {
    pub local_only: Vec<SessionInfo>,
    pub remote_only: Vec<SessionInfo>,
    pub diverged: Vec<SessionInfo>,
    pub in_sync: u32,
}

fn tar_directory(dir: &Path) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let encoder = GzEncoder::new(&mut buf, Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        archive.follow_symlinks(false);
        let dir_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "data".to_string());
        archive.append_dir_all(&dir_name, dir)?;
        archive.into_inner()?.finish()?;
    }
    Ok(buf)
}

fn untar_directory(data: &[u8], target: &Path) -> Result<()> {
    let decoder = GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);
    if target.exists() {
        std::fs::remove_dir_all(target)?;
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    archive.unpack(target.parent().unwrap_or(target))?;
    Ok(())
}

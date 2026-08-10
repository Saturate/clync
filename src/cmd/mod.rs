pub mod checkout;
pub mod init;
pub mod join;
pub mod sync_cmd;

pub use sync_cmd::{do_pull, do_push};

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

use crate::config::{self, Config, EncryptionConfig};
use crate::scanner::ScanFilter;
use crate::synclog;

pub fn build_filter(max_age: Option<u64>, max_size: Option<u64>) -> ScanFilter {
    ScanFilter {
        max_age_days: max_age,
        max_file_size: max_size,
    }
}

pub fn cmd_list(
    query: Option<String>,
    max_age: Option<u64>,
    limit: usize,
    json: bool,
) -> Result<()> {
    let config = Config::load()?;
    let filter = ScanFilter {
        max_age_days: max_age,
        max_file_size: None,
    };

    let sessions =
        crate::list::list_sessions(&config.claude_projects_dir(), query.as_deref(), &filter)?;
    let limited: Vec<_> = sessions.into_iter().take(limit).collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&limited)?);
        return Ok(());
    }

    if limited.is_empty() {
        println!("no sessions found");
        return Ok(());
    }

    for s in &limited {
        let age = format_age(s.mtime);
        let size = format_size(s.size_bytes);
        let preview = s
            .first_message
            .as_deref()
            .unwrap_or("(no user message)")
            .replace('\n', " ");
        let preview = truncate_safe(&preview, 80);

        println!(
            "{} [{}] {} | {} msgs | {}",
            short_uuid(&s.uuid),
            s.project,
            age,
            s.messages,
            size
        );
        println!("  {preview}");
    }

    Ok(())
}

pub fn cmd_log(limit: usize, json: bool) -> Result<()> {
    let config = Config::load()?;
    let store_path = config.storage_path().ok_or_else(|| {
        anyhow::anyhow!("log requires local storage (not available with S3 backend)")
    })?;
    let entries = synclog::read_recent(store_path, limit)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("no sync history yet");
        return Ok(());
    }

    for entry in &entries {
        let age = format_age(entry.timestamp);
        let mut parts = Vec::new();
        if entry.sessions_pushed > 0 {
            parts.push(format!("{} pushed", entry.sessions_pushed));
        }
        if entry.sessions_pulled > 0 {
            parts.push(format!("{} pulled", entry.sessions_pulled));
        }
        if entry.sessions_merged > 0 {
            parts.push(format!("{} merged", entry.sessions_merged));
        }
        if entry.extras > 0 {
            parts.push(format!("{} extras", entry.extras));
        }
        let summary = if parts.is_empty() {
            "no changes".into()
        } else {
            parts.join(", ")
        };
        println!(
            "{} {} [{}] {}",
            age, entry.operation, entry.machine, summary
        );
    }

    Ok(())
}

pub fn cmd_config(action: Option<super::ConfigAction>) -> Result<()> {
    let action = action.unwrap_or(super::ConfigAction::Show);
    match action {
        super::ConfigAction::Show => {
            let path = Config::config_path()?;
            if !path.exists() {
                bail!("no config found. Run `clync init` first.");
            }
            let config = Config::load()?;
            let enc_method = match &config.encryption {
                EncryptionConfig::KeyFile { path } => format!("key_file ({})", path.display()),
                EncryptionConfig::Passphrase { env_var } => format!("passphrase (${env_var})"),
                EncryptionConfig::OnePassword { reference } => format!("1password ({reference})"),
                EncryptionConfig::Bitwarden { item_id, .. } => format!("bitwarden ({item_id})"),
                EncryptionConfig::Pass { entry } => format!("pass ({entry})"),
                EncryptionConfig::None => "none (plain text)".into(),
            };
            let t = &config.targets;
            let storage_desc = match &config.sync.storage {
                crate::config::StorageConfig::Git {
                    path, auto_push, ..
                } => {
                    format!("git ({}), auto_push: {auto_push}", path.display())
                }
                crate::config::StorageConfig::Folder { path } => {
                    format!("folder ({})", path.display())
                }
                #[cfg(feature = "s3")]
                crate::config::StorageConfig::S3 { bucket, region, .. } => {
                    format!("s3 ({bucket}, {region})")
                }
            };
            println!("storage:         {storage_desc}");
            println!("claude dir:      {}", config.sync.claude_dir.display());
            println!("encryption:      {enc_method}");
            println!("companion dirs:  {}", config.sync.include_companion_dirs);
            if config.sync.storage.is_git() {
                let lfs = config.sync.storage.lfs_threshold();
                let lfs_display = if lfs == 0 {
                    "disabled".to_string()
                } else {
                    format!("{}MB threshold", lfs / (1024 * 1024))
                };
                println!("git lfs:         {lfs_display}");
            }
            println!();
            println!("targets:");
            println!("  sessions:        {}", t.sessions);
            println!("  memories:        {}", t.memories);
            println!("  settings:        {}", t.settings);
            println!("  commands:        {}", t.commands);
            println!("  skills:          {}", t.skills);
            println!("  global CLAUDE.md: {}", t.global_claude_md);
        }
        super::ConfigAction::Edit => {
            let path = Config::config_path()?;
            if !path.exists() {
                bail!("no config found. Run `clync init` first.");
            }
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
            std::process::Command::new(&editor)
                .arg(&path)
                .status()
                .with_context(|| format!("failed to open {editor}"))?;
        }
        super::ConfigAction::Path => {
            let path = Config::config_path()?;
            println!("{}", path.display());
        }
        super::ConfigAction::Set { key, value } => {
            let path = Config::config_path()?;
            if !path.exists() {
                bail!("no config found. Run `clync init` first.");
            }
            let contents = std::fs::read_to_string(&path)?;
            let mut doc: toml::Table = toml::from_str(&contents).context("invalid config")?;

            set_toml_value(&mut doc, &key, &value)?;

            let new_contents = toml::to_string_pretty(&doc)?;

            let _: Config = toml::from_str(&new_contents)
                .context("invalid config after set. Check key and value types.")?;

            std::fs::write(&path, new_contents)?;
            println!("set {key} = {value}");
        }
    }
    Ok(())
}

fn set_toml_value(table: &mut toml::Table, key: &str, value: &str) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() < 2 {
        bail!("key must be in section.field format (e.g. targets.skills, sync.git.lfs_threshold)");
    }

    let field = parts[parts.len() - 1];
    let mut current = table;
    for &section in &parts[..parts.len() - 1] {
        current = current
            .get_mut(section)
            .and_then(|v| v.as_table_mut())
            .with_context(|| format!("section '{section}' not found"))?;
    }

    let parsed: toml::Value = if value == "true" {
        toml::Value::Boolean(true)
    } else if value == "false" {
        toml::Value::Boolean(false)
    } else if let Some(bytes) = parse_byte_size(value) {
        toml::Value::Integer(bytes as i64)
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else {
        toml::Value::String(value.to_string())
    };

    current.insert(field.to_string(), parsed);
    Ok(())
}

pub(crate) fn parse_byte_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, suffix) = if s.ends_with("GB") || s.ends_with("gb") {
        (s[..s.len() - 2].trim(), 1024 * 1024 * 1024)
    } else if s.ends_with("MB") || s.ends_with("mb") {
        (s[..s.len() - 2].trim(), 1024 * 1024)
    } else if s.ends_with("KB") || s.ends_with("kb") {
        (s[..s.len() - 2].trim(), 1024)
    } else {
        return None;
    };
    num.parse::<u64>().ok().map(|n| n * suffix)
}

pub(crate) fn short_uuid(uuid: &str) -> &str {
    let mut end = uuid.len().min(8);
    while end > 0 && !uuid.is_char_boundary(end) {
        end -= 1;
    }
    &uuid[..end]
}

fn truncate_safe(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub(3);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

pub(crate) fn format_age(mtime: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let diff = now.saturating_sub(mtime);
    if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

pub fn cmd_mv(uuid_prefix: &str, target: &str) -> Result<()> {
    let claude_dir = config::home_dir()
        .context("cannot determine home directory")?
        .join(".claude");
    let projects_dir = claude_dir.join("projects");

    let filter = ScanFilter::default();
    let sessions = crate::scanner::scan_sessions(&projects_dir, &filter)?;

    let matches: Vec<_> = sessions
        .iter()
        .filter(|s| s.uuid.starts_with(uuid_prefix))
        .collect();

    if matches.is_empty() {
        bail!("no session matching '{uuid_prefix}'");
    }
    let session = if matches.len() == 1 {
        matches[0]
    } else {
        let same_uuid = matches.windows(2).all(|w| w[0].uuid == w[1].uuid);
        if !same_uuid {
            for s in &matches {
                println!("  {} [{}]", short_uuid(&s.uuid), s.entry.project_path);
            }
            bail!(
                "ambiguous prefix '{uuid_prefix}', {} distinct sessions match",
                matches.len()
            );
        }
        let home = config::home_dir()
            .map(|h| h.to_string_lossy().replace('/', "-"))
            .unwrap_or_default();
        let home_prefix = format!("{}-", home.trim_start_matches('-'));
        let local = matches.iter().find(|s| {
            let dir = s.project_dir_name.trim_start_matches('-');
            dir.starts_with(&home_prefix)
        });
        if let Some(s) = local {
            s
        } else {
            for s in &matches {
                println!("  {} [{}]", short_uuid(&s.uuid), s.entry.project_path);
            }
            bail!(
                "ambiguous prefix '{uuid_prefix}', {} matches",
                matches.len()
            );
        }
    };
    let target_path = config::expand_path(&PathBuf::from(target));
    let encoded = target_path.to_string_lossy().replace('/', "-");

    let target_dir = projects_dir.join(&encoded);
    std::fs::create_dir_all(&target_dir)?;

    let src_jsonl = &session.jsonl_path;
    let dst_jsonl = target_dir.join(format!("{}.jsonl", session.uuid));

    if dst_jsonl.exists() {
        bail!(
            "session {} already exists in target project",
            short_uuid(&session.uuid)
        );
    }

    std::fs::rename(src_jsonl, &dst_jsonl)?;

    if let Some(ref companion) = session.companion_dir
        && companion.exists()
    {
        let dst_companion = target_dir.join(&session.uuid);
        std::fs::rename(companion, &dst_companion)?;
    }

    println!(
        "moved {} from [{}] to [{}]",
        short_uuid(&session.uuid),
        session.entry.project_path,
        encoded
    );

    let src_project_dir = src_jsonl.parent().unwrap_or(std::path::Path::new("."));
    let src_memory_dir = src_project_dir.join("memory");
    if src_memory_dir.exists() {
        let memory_files = find_session_memories(&dst_jsonl, &src_memory_dir);
        if !memory_files.is_empty() {
            let dst_memory_dir = target_dir.join("memory");
            std::fs::create_dir_all(&dst_memory_dir)?;
            for file in &memory_files {
                let name = file.file_name().unwrap_or_default();
                let dst = dst_memory_dir.join(name);
                if !dst.exists() {
                    std::fs::rename(file, &dst)?;
                    println!("  moved memory: {}", name.to_string_lossy());
                }
            }
            update_memory_index(&src_memory_dir, &memory_files)?;
        }
    }

    Ok(())
}

fn find_session_memories(
    session_jsonl: &std::path::Path,
    memory_dir: &std::path::Path,
) -> Vec<PathBuf> {
    let content = match std::fs::read_to_string(session_jsonl) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut found = Vec::new();
    let entries = match std::fs::read_dir(memory_dir) {
        Ok(e) => e,
        Err(_) => return found,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "MEMORY.md" {
            continue;
        }
        let search = format!("/memory/{name}");
        if content.contains(&search) {
            found.push(entry.path());
        }
    }
    found
}

fn update_memory_index(memory_dir: &std::path::Path, moved_files: &[PathBuf]) -> Result<()> {
    let index_path = memory_dir.join("MEMORY.md");
    if !index_path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&index_path)?;
    let moved_names: Vec<String> = moved_files
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .collect();

    let filtered: String = content
        .lines()
        .filter(|line| !moved_names.iter().any(|name| line.contains(name)))
        .map(|line| format!("{line}\n"))
        .collect();

    std::fs::write(&index_path, filtered)?;
    Ok(())
}

pub(crate) fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

//! JSONL helpers shared by the Claude and pi formats.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

/// Read every parseable line. Bad lines are reported and skipped, since a
/// single truncated write shouldn't make a whole session unreadable.
pub fn read_values(path: &Path) -> Result<Vec<Value>> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut values = Vec::new();
    // Split on raw bytes: a write cut off mid-character leaves invalid UTF-8,
    // which should cost one line, not the session.
    for line in BufReader::new(file).split(b'\n') {
        let line = line?;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(&line) {
            Ok(v) => values.push(v),
            Err(e) => eprintln!(
                "warning: skipping unparseable line in {}: {}",
                path.display(),
                e
            ),
        }
    }
    Ok(values)
}

/// Read up to `n` parseable lines from the start of a file.
pub fn read_head(path: &Path, n: usize) -> Vec<Value> {
    let Ok(file) = fs::File::open(path) else {
        return vec![];
    };
    BufReader::new(file)
        .lines()
        .take(n)
        .map_while(|l| l.ok())
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

pub fn write_line(out: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Epoch ms from an ISO `timestamp` field, falling back to `message.timestamp` (ms).
pub fn entry_timestamp(entry: &Value) -> Option<u64> {
    if let Some(ts) = entry.get("timestamp").and_then(|v| v.as_str())
        && let Ok(dt) = ts.parse::<chrono::DateTime<chrono::Utc>>()
    {
        return u64::try_from(dt.timestamp_millis()).ok();
    }
    entry
        .get("message")
        .and_then(|m| m.get("timestamp"))
        .and_then(|v| v.as_u64())
}

pub fn iso_millis(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Text of all `{type: "text"}` blocks in a content array, newline-joined.
pub fn text_blocks(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Write to a temp file and rename into place so a failed conversion never
/// leaves a half-written session for the target tool to trip over.
pub fn write_atomic(path: &Path, lines: &[Value]) -> Result<()> {
    let tmp = path.with_extension("jsonl.tmp");
    let written = (|| -> Result<()> {
        let mut out = std::io::BufWriter::new(fs::File::create(&tmp)?);
        for line in lines {
            write_line(&mut out, line)?;
        }
        out.flush()?;
        // Sync before the rename so a crash can't leave an empty file under
        // the final name.
        out.get_ref().sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if written.is_err() {
        fs::remove_file(&tmp).ok();
    }
    written
}

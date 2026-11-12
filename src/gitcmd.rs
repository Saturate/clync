use std::process::Command;

/// A `git` command for every clync git call.
///
/// Git for Windows defaults to `core.longpaths=false`, which caps paths at
/// 260 chars. Synced skill dirs (with `node_modules`) easily exceed that
/// once the repo's own path is added, and checkout then fails half-way.
pub fn git() -> Command {
    let mut cmd = Command::new("git");
    if cfg!(windows) {
        cmd.args(["-c", "core.longpaths=true"]);
    }
    cmd
}

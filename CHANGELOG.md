# Changelog

All notable changes to clync will be documented in this file.


## 0.5.0 - 2026-10-07

Switch from cargo-changeset to changesetter for release management

Version bump to 0.5.0 for compression and batch move features

## 0.4.1 - 2026-08-10

### Added

- Checkout command with TUI for discovering and cloning unmapped project repos
- `clone_base` config option for default clone directory
- Remote URLs stored in manifest during push for cross-machine project discovery

### Fixed

- Default config directory to `~/.clync/` instead of `~/.config/clync/`, with fallback for existing installs
- Silently skip orphaned manifest entries on pull and report as "archived" instead of warning

## 0.4.0 - 2026-07-08

### Added

- Support for multiple storage backends: git (default), local folder (NAS/Dropbox/USB), and S3-compatible cloud storage (AWS, R2, MinIO)
- Move sessions between project directories with `clync mv`

## 0.3.0

### Added

- Auto-track large session files with git-lfs when they exceed the configured threshold (default 99 MB)
- `[sync.git]` config section for storage provider settings
- `config set` now supports nested keys (e.g. `sync.git.lfs_threshold 50MB`) and human-readable byte sizes

### Changed

- Extracted shared file helpers into `fileutil` module, removing ~120 lines of duplication
- Split `main.rs` into focused `cmd/` modules with CI-enforced file length limits

## 0.2.3

### Fixed

- Memories now use normalized project paths for cross-machine sync, matching how sessions work
- MEMORY.md index files are union-merged on pull instead of overwritten, so entries from different machines combine
- Auto-migrates from old `extras/memories/` layout on first push/pull

## 0.2.2

### Fixed

- Verify existing repo remote matches on join
- Reuse existing repo on join instead of failing

## 0.2.1

### Fixed

- Clean up on join failure
- Allow editing 1Password reference during join

## 0.2.0

### Added

- Reset command to remove clync config

## 0.1.8

### Fixed

- Better error messages on decryption failures

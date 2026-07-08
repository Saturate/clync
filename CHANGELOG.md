# Changelog

All notable changes to clync will be documented in this file.

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

## [0.4.0] - 2026-07-08
### Added

- **clync**: Support for multiple storage backends: git (default), local folder (NAS/Dropbox/USB), and S3-compatible cloud storage (AWS, R2, MinIO)
- **clync**: Move sessions between project directories with clync mv

### Changed

- **clync**: Internal architectural changes

[0.4.0]: https://github.com/Saturate/clync/compare/v0.3.0...v0.4.0

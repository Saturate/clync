---
category: added
clync: minor
---
Compress data with zstd before encryption, reducing sync repo size by ~65-70%. Configure with `compression` and `compression_level` in config. Pull auto-detects both formats for backward compatibility.

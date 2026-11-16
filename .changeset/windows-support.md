---
clync: minor
---
Windows support: session, memory and `mv` paths now match how Claude Code names project folders on every platform (`C:\code\app` → `C--code-app`, and dots, spaces and underscores become `-` too), so sessions sync to the right place between Windows, macOS and Linux. The age key file is restricted to the current user on Windows, and release builds now include a Windows binary.

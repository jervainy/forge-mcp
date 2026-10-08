# ForgeMCP Release History

This file tracks published versions, their immutable Git tags, source commits, and binary distribution targets.

| Version | Git tag | Source commit | Published (UTC) | GitHub Actions |
| --- | --- | --- | --- | --- |
| **0.1.0** | [`v0.1.0`](https://github.com/jervainy/forge-mcp/tree/v0.1.0) | [`f27481f25f5738ff58db70a33fe9667adb75041a`](https://github.com/jervainy/forge-mcp/commit/f27481f25f5738ff58db70a33fe9667adb75041a) | 2026-10-08 | [Release binaries #37734924997](https://github.com/jervainy/forge-mcp/actions/runs/37734924997) |

## v0.1.0

- **Release:** https://github.com/jervainy/forge-mcp/releases/tag/v0.1.0
- **Tag:** `refs/tags/v0.1.0` (lightweight Git tag pointing to source commit above)
- **Packages:**
  - `forge-mcp-v0.1.0-x86_64-apple-darwin.tar.gz`
  - `forge-mcp-v0.1.0-aarch64-apple-darwin.tar.gz`
  - `forge-mcp-v0.1.0-x86_64-pc-windows-msvc.zip`
  - `forge-mcp-v0.1.0-aarch64-pc-windows-msvc.zip`
  - `forge-mcp-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`
  - `forge-mcp-v0.1.0-aarch64-unknown-linux-gnu.tar.gz`
  - `SHA256SUMS.txt`
- **Validation:** all six native jobs built and executed `forge-mcp --help` successfully; checksum file and six-package completeness check passed.

New releases should retain their tags and append a record here. The release workflow is in `.github/workflows/release.yml`, triggered by a `release/vX.Y.Z` branch whose version matches `Cargo.toml`.

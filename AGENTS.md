# AGENTS.md

## What this is

Standalone Linux Bilibili MCP server (`weiyu-mcp`) extracted from the Windows desktop app
[mkktop/bilbili_copy](https://github.com/mkktop/bilbili_copy). Rust, no Tauri/WebView/FFmpeg/SQLite.

- `crates/weiyu-core/` — copied Bilibili API layer (`wbi`, risk control, video, search, comments,
  subtitles, danmaku, article, personal lists, interaction writes).
- `crates/weiyu-mcp/` — server runtime: CLI, tool registry, stdio/HTTP MCP transports, auth,
  embedded admin UI (`/admin`), settings/cookie persistence.

## Commands

```bash
cargo fmt -p weiyu-mcp --check
cargo clippy -p weiyu-mcp --all-targets --no-deps -- -D warnings
cargo test --workspace --no-default-features --locked
cargo build --release -p weiyu-mcp
cargo run -q -p weiyu-mcp -- tools
```

Cross-compilation and CI are in `.github/workflows/ci.yml`; release binaries and the multi-arch
GHCR image are built in `.github/workflows/release.yml` (`deploy/Dockerfile.ci`).

## Tool model

- 35 read-only tools are always registered.
- 5 interaction tools (`like_video`, `coin_video`, `favorite_video`, `add_watch_later`,
  `remove_watch_later`) are only registered when the runtime setting `allow_write=true`.
- Runtime settings live in `$WEIYU_DATA_DIR/settings.json`; the admin UI writes them. CLI/env values
  only seed the file on first start. Rate/timeout/write changes are hot; concurrency needs a restart.
- Do not make write tools visible by default. MCP clients send the full `tools/list` to the model;
  hiding them is the safety boundary, not a runtime rejection.
- Write tools require `bili_jct` in the cookie. Read-only tools can work with `SESSDATA` only.

## Admin UI and cookies

- `/admin` serves the embedded single-page UI; `/admin/api/*` requires a separate admin Bearer token
  (`WEIYU_ADMIN_TOKEN` / `--admin-token-file`). Do not reuse the MCP token for admin.
- QR login writes `$WEIYU_DATA_DIR/bilibili.cookie` and hot-swaps the in-memory credential.
- A data-dir cookie wins over the read-only `WEIYU_COOKIE_FILE` fallback. An empty data-dir cookie is
  a logout marker and suppresses the fallback.
- Settings/cookie files are written atomically with 0600. Keep the data dir writable only by the
  service user / container UID.

## Core copy policy

`crates/weiyu-core` is a copy, not a dependency. Sync with:

```bash
scripts/sync-core.sh /path/to/bilbili_copy
```

Put shared API fixes in the upstream desktop repo first, then sync, then run the full test suite.
Avoid server-only changes in `weiyu-core` unless they are also acceptable to the desktop app.

## Secrets

- Cookie comes from `$WEIYU_DATA_DIR/bilibili.cookie` (writable, QR login) or a read-only
  `WEIYU_COOKIE_FILE` / `--cookie-file` fallback.
- Never accept cookies as tool arguments, return them, or log them.
- HTTP MCP requires a Bearer token (≥32 chars) via `WEIYU_MCP_TOKEN` or `--token-file`.
- The admin UI requires a separate Bearer token (≥32 chars) via `WEIYU_ADMIN_TOKEN` or
  `--admin-token-file`; without it `/admin` is disabled but `/mcp` keeps working.
- Host/Origin validation and SSRF guards live in `src/http.rs`, `src/auth.rs` and `src/input.rs`;
  keep them.

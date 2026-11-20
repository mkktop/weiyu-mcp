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

- 39 read-only tools are always registered (including the message-feed tools `get_at_feed`,
  `get_reply_feed`, `get_like_feed`, `get_message_unread` for @/reply/like notifications).
- 5 interaction tools (`like_video`, `coin_video`, `favorite_video`, `add_watch_later`,
  `remove_watch_later`) are only registered when the runtime setting `allow_write=true`.
- `post_comment` is gated separately by `allow_comment` (default false), independent of
  `allow_write`; it supports replies (`parent`/`root`) and real @ mentions (`mentions` →
  `at_name_to_mid`). Keep it off unless the operator explicitly wants public posting.
- Runtime settings live in `$WEIYU_DATA_DIR/settings.json`; the admin UI writes them. CLI/env values
  only seed the file on first start. Rate/timeout/write/comment changes are hot; concurrency needs a
  restart.
- Do not make write tools visible by default. MCP clients send the full `tools/list` to the model;
  hiding them is the safety boundary, not a runtime rejection.
- Write tools require `bili_jct` in the cookie. Read-only tools can work with `SESSDATA` only.

## Admin UI and cookies

- `/admin` serves the embedded single-page UI. First access asks the operator to set a management
  password, default `admin`; the Argon2 hash is stored in `$WEIYU_DATA_DIR/admin.json`.
- Password login creates a 12-hour in-memory session token. The UI sends it as a Bearer token; the
  plaintext password is never stored client-side.
- `WEIYU_ADMIN_TOKEN` / `--admin-token-file` is an optional admin API Bearer for scripts, not the UI
  login credential. Do not route the UI through it.
- QR login writes `$WEIYU_DATA_DIR/bilibili.cookie` and hot-swaps the in-memory credential.
- The settings card has two independent switches: interaction writes and comment writes. Comment
  posting is public and notifies replied/mentioned users; never enable it by default.
- A data-dir cookie wins over the read-only `WEIYU_COOKIE_FILE` fallback. An empty data-dir cookie is
  a logout marker and suppresses the fallback.
- Settings/cookie/admin files are written atomically with 0600. Keep the data dir writable only by the
  service user / container UID.
- Changing the password invalidates all sessions and returns a fresh one. Forgetting the password is
  recovered by deleting `admin.json` and restarting.

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
- The admin UI uses a password (`admin.json`, Argon2) and a session token; `WEIYU_ADMIN_TOKEN` or
  `--admin-token-file` is an optional API Bearer for scripts. Without it the UI still works.
- Host/Origin validation and SSRF guards live in `src/http.rs`, `src/auth.rs`, `src/admin_auth.rs`
  and `src/input.rs`; keep them.

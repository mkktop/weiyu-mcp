# AGENTS.md

## What this is

Standalone Linux Bilibili MCP server (`weiyu-mcp`) extracted from the Windows desktop app
[mkktop/bilbili_copy](https://github.com/mkktop/bilbili_copy). Rust, no Tauri/WebView/FFmpeg/SQLite.

- `crates/weiyu-core/` — copied Bilibili API layer (`wbi`, risk control, video, search, comments,
  subtitles, danmaku, article, personal lists, interaction writes).
- `crates/weiyu-mcp/` — server runtime: CLI, tool registry, stdio/HTTP MCP transports, auth.

## Commands

```bash
cargo fmt -p weiyu-mcp --check
cargo clippy -p weiyu-mcp --all-targets --no-deps -- -D warnings
cargo test --workspace --no-default-features --locked
cargo build --release -p weiyu-mcp
cargo run -q -p weiyu-mcp -- tools
```

Cross-compilation and CI are in `.github/workflows/ci.yml`.

## Tool model

- 35 read-only tools are always registered.
- 5 interaction tools (`like_video`, `coin_video`, `favorite_video`, `add_watch_later`,
  `remove_watch_later`) are only registered when `--allow-write` / `WEIYU_ALLOW_WRITE=true`.
- Do not make write tools visible by default. MCP clients send the full `tools/list` to the model;
  hiding them is the safety boundary, not a runtime rejection.
- Write tools require `bili_jct` in the cookie. Read-only tools can work with `SESSDATA` only.

## Core copy policy

`crates/weiyu-core` is a copy, not a dependency. Sync with:

```bash
scripts/sync-core.sh /path/to/bilbili_copy
```

Put shared API fixes in the upstream desktop repo first, then sync, then run the full test suite.
Avoid server-only changes in `weiyu-core` unless they are also acceptable to the desktop app.

## Secrets

- Cookie comes only from `WEIYU_COOKIE_FILE` / `--cookie-file`.
- Never accept cookies as tool arguments, return them, or log them.
- HTTP MCP always requires a Bearer token (≥32 chars) via `WEIYU_MCP_TOKEN` or `--token-file`.
- Host/Origin validation and SSRF guards live in `src/http.rs` and `src/input.rs`; keep them.

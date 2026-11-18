# Upstream

`crates/weiyu-core` is a copy of the shared Bilibili API crate from:

- Repository: https://github.com/mkktop/bilbili_copy
- Source path: `crates/weiyu-core`
- Copied from commit: `031bc28` (`031bc28c926a9807eef885f46dcbcdc5f3455889`)

The copy is intentional: this project must build and deploy independently of the
desktop application. Keep the copy in sync with:

```bash
scripts/sync-core.sh /path/to/bilbili_copy
```

Review the diff before committing a sync; desktop-specific behavior (Windows DPAPI
credentials) must not leak into the server runtime.

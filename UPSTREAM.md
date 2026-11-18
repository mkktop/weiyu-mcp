# Upstream

`crates/weiyu-core` is a copy of the shared Bilibili API crate from:

- Repository: https://github.com/mkktop/bilbili_copy
- Source path: `crates/weiyu-core`
- Copied from commit: `2535359370db464fb82e18fc345150264bee0837`

The copy is intentional: this project must build and deploy independently of the
desktop application. Keep the copy in sync with:

```bash
scripts/sync-core.sh /path/to/bilbili_copy
```

The sync script records the new upstream commit in this file. Review the diff
before committing a sync; desktop-specific behavior (Windows DPAPI credentials)
must not leak into the server runtime.

#!/usr/bin/env bash
set -euo pipefail

upstream="${1:?Usage: scripts/sync-core.sh /path/to/bilbili_copy}"
source_dir="$upstream/crates/weiyu-core"
test -d "$source_dir" || { echo "Not a bilbili_copy checkout: $upstream" >&2; exit 2; }

rev="$(git -C "$upstream" rev-parse HEAD)"
short="$(git -C "$upstream" rev-parse --short HEAD)"

rm -rf crates/weiyu-core
cp -r "$source_dir" crates/weiyu-core

cat > UPSTREAM.md <<EOF
# Upstream

\`crates/weiyu-core\` is a copy of the shared Bilibili API crate from:

- Repository: https://github.com/mkktop/bilbili_copy
- Source path: \`crates/weiyu-core\`
- Copied from commit: \`$short\` (\`$rev\`)

The copy is intentional: this project must build and deploy independently of the
desktop application. Keep the copy in sync with:

\`\`\`bash
scripts/sync-core.sh /path/to/bilbili_copy
\`\`\`

Review the diff before committing a sync; desktop-specific behavior (Windows DPAPI
credentials) must not leak into the server runtime.
EOF

echo "Synced weiyu-core from $short"
echo "Review: git diff -- crates/weiyu-core UPSTREAM.md"

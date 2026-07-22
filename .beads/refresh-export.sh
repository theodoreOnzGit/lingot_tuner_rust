#!/usr/bin/env bash
# Refresh the committed bead export (.beads/issues.export.jsonl).
#
# Why this exists: bd (beads-rs 0.1.26) manages `.beads/issues.jsonl` itself and
# forcibly makes it a symlink into ~/.local/share/beads-rs/exports/<hash>/ on every
# sync. If you replace that symlink with a real file, bd renames yours to
# `issues.jsonl.bak` and recreates the symlink. So `.beads/issues.jsonl` is
# gitignored, and this script copies its *contents* to a second path that bd never
# touches, which is what actually gets committed.
#
# Run this after changing beads and before committing:
#     .beads/refresh-export.sh && git add .beads/issues.export.jsonl
set -euo pipefail

repo_root="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
src="$repo_root/.beads/issues.jsonl"
dst="$repo_root/.beads/issues.export.jsonl"

# Flush any debounced writes so the export on disk is current.
bd --repo "$repo_root" sync >/dev/null

if [ ! -r "$src" ]; then
  echo "error: cannot read $src" >&2
  echo "hint: run any bd command in this repo (e.g. 'bd list') to recreate it." >&2
  exit 1
fi

# cp follows the symlink, so $dst is always a regular file with real content.
cp "$src" "$dst"
echo "refreshed $dst ($(wc -l < "$dst") beads)"

# How bead history is stored in this repo

Three things are easy to confuse. In order of authority:

## 1. `refs/heads/beads/store` — the real store (already versioned, already pushed)

bd's canonical data lives in this repo as an **independent git branch**, `beads/store`,
holding `state.jsonl`, `deps.jsonl`, `tombstones.jsonl`, `meta.json`. It is not an
ancestor of `main`; it is its own history. It is already pushed to
`origin/beads/store`.

```sh
git log --oneline beads/store
git show beads/store:state.jsonl
```

This means bead history is **not** confined to one machine. A fresh clone with `bd`
installed recovers everything from the remote — `bd` fetches `refs/heads/beads/store`
on init/sync. Losing the laptop does not lose the beads.

`beads/store` is under `refs/heads/`, so the default clone/fetch refspec picks it up
like any other branch — no special configuration needed.

## 2. `.beads/issues.jsonl` — machine-local, gitignored

This is bd's "Go-compat" convenience export. bd **owns** this path and recreates it as
a symlink to
`~/.local/share/beads-rs/exports/<sha256(normalized-remote-url)[:16]>/issues.jsonl`
after every sync.

If you replace it with a regular file, bd renames yours to `issues.jsonl.bak` and puts
the symlink back (`compat::export::ensure_symlinks`). There is no configuration to
redirect it — the location is derived from `BD_DATA_DIR`/`XDG_DATA_HOME`, which are
global to all repos, not per-repo. So this path is gitignored.

## 3. `.beads/issues.export.jsonl` — the committed, human-readable snapshot

A real file (mode 100644) with real JSON content, at a path bd never touches. This is
what makes bead content readable on GitHub and diffable in normal `main` commits.

It is a derived snapshot, not the source of truth. If it goes stale, regenerate it —
never hand-edit it.

### Keeping it fresh automatically

`.githooks/pre-commit` refreshes and stages this file on every commit. Enable it once
per clone:

```sh
git config core.hooksPath .githooks
```

The hook is deliberately **non-fatal**: refreshing runs `bd sync`, which touches the
network (bd auto-pushes its own `beads/store`), and neither an offline machine nor a
missing `bd` should ever block a commit. If it cannot refresh, it warns and lets the
commit through — so treat a stale export as possible, not impossible.

Without the hook, refresh manually:

```sh
.beads/refresh-export.sh
git add .beads/issues.export.jsonl
```

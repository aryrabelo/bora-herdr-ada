# DOX-OMP install log

## 2026-07-16 — initial install

Installed the DOX-OMP rail.

**Changed files:**

- `AGENTS.md` — **wrapped** in the DOX shape (title `# DOX framework — bora`,
  Project, Core Contract, Read Before Editing, Verification, Global Contracts,
  Closeout, Child DOX Index). Every pre-existing section was preserved verbatim
  and relocated under the appropriate DOX heading: Scope and Audience, Universal
  Project Rules, Maintainer Workflow, Local Can Machine Workflow, Agent
  Detection Updates, Vendored libghostty-vt, Docs, Release Channels, and
  External contributor guardrail. The prior "Testing" section became
  "Verification"; "Commit Style" and "Code Conventions" became subsections of
  "Global Contracts". No user content deleted.
- `CLAUDE.md` — **left untouched** (already a symlink to `AGENTS.md`).
- `.omp/RULES.md` — **created** (sticky DOX pass rules).
- `.omp/INSTALL-LOG.md` — **created** (this file).

**Rules load globally.** The shared OMP rules live in `~/.omp/agent/rules/` and
are injected globally; no `.omp/rules/*` copied into this repo (project copies
would just shadow the global install).

**Note (not fixed):** AGENTS.md content still uses the upstream `herdr` /
`ogulcancelik` identity. This install did NOT rename anything — reported only.

**Rollback:**

- `git revert <this commit>` — find the hash with `git log --oneline -1 -- .omp`.
- Or restore the pre-install state directly:
  `git checkout <hash>^ -- AGENTS.md CLAUDE.md && rm -rf .omp`

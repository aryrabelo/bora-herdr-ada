---
description: "Never write scratch/temp files under /tmp; use ~/Sites/temp-files/<timestamp>/ instead. Read before any bash command or tool call that creates scratch files, temp dirs, or downloads (mktemp, /tmp/..., os.tmpdir())."
---

# No writes under /tmp

`/tmp` is out of bounds for anything this agent creates — scratch files,
downloads, generated scripts, exported artifacts, `mktemp` output. Applies in
every OMP profile (default and work), on every machine.

- Scratch dir: `~/Sites/temp-files/$(date +%Y%m%d-%H%M%S)/` — one fresh
  timestamped directory per task/session, `mkdir -p` it before first write.
- `mktemp` calls: pass `-p ~/Sites/temp-files/<timestamp>` (or set
  `TMPDIR=~/Sites/temp-files/<timestamp>` for that one command), never bare
  `mktemp` / `mktemp -d` which default to `/tmp`.
- Never `cd /tmp`, write files under `/tmp/...`, or point a tool's output
  path at `/tmp`.
- These directories are not auto-cleaned — do not treat them as ephemeral in
  a way that requires deletion; leftover run artifacts under
  `~/Sites/temp-files/` are expected and fine to accumulate.

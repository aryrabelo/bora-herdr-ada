---
description: "How to save tokens in OMP: subagent fan-out, per-model dispatch via eval agent(), completion() for micro-tasks, and why agent-to-agent chat is expensive. Read before heavy exploration, bulk work, or multi-model dispatch."
---

# OMP token economy

OMP already ships every mechanism needed — nothing to build. Pick the cheapest
tool that fits:

## 1. `task` subagents — parallel fan-out

- Subagents start with a CLEAN context: the expensive main session never pays
  for their exploration (greps, file reads, test output).
- No per-subagent model choice — all run on their agent type's model. Only
  `scout` is routed to a faster/cheaper model: use `agent: "scout"` for all
  read-only research.
- Economics: delegate when (exploration/execution tokens) >> (briefing +
  report tokens). Below ~2k tokens of work, the handoff overhead eats the gain
  — do it inline.

## 2. `eval` + `agent(prompt, model=...)` — real multi-model dispatch

- The only way to pin a specific model per unit of work. Prefer the ROLE tiers
  `"smol"` / `"default"` / `"slow"` — they resolve through `modelRoles` in
  `~/.omp/agent/config.yml`, the single place model choices live (model names
  churn monthly; rules and skills must never hardcode them).
- Only pin a concrete id when a specific provider matters (quota pool, rate
  tier); then use a BARE id verified verbatim against `~/.omp/agent/models.db`
  first — never guess or truncate.
- Pattern: write shared context to `local://ctx.md` once; wrap each dispatch in
  try/except; run the wave with `parallel()`; collect short reports; synthesize
  in the main session.
- Route mechanical/volume work (boilerplate, data collection, test runs) to
  cheap models; keep intent, decomposition, and taste in the main model.

## 3. `completion(prompt, model="smol")` — cheapest of all

- One-shot call with NO agent harness. Use for pure-text micro-tasks:
  classify, summarize, extract, reformat. Zero tool overhead.

## 4. `hub` — agent messaging (use sparingly)

- Enables live agent-to-agent conversation, but every message reprocesses the
  recipient's accumulated context — multi-turn dialogue between agents grows
  ~quadratically in cost.
- Default to fan-out WITHOUT conversation: one briefing → isolated work →
  short reports → single synthesis. Reserve hub chat for genuine cross-slice
  dependencies a better briefing cannot remove, or for deliberate
  quality-over-cost patterns (adversarial review, debate).

## Rules of thumb

- Exploration-heavy task: expect 40–70% savings from delegation.
- Small direct edit: delegation is net negative — do it inline.
- Never let the expensive main session read bulk output a subagent could
  summarize.

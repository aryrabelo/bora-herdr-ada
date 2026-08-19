---
alwaysApply: true
---

# NEXT ACTION protocol

End every recommendation-bearing response with `## NEXT ACTION` and exactly one tag:

- `[DONE]` — you already executed it. Auto-apply ONLY when all three hold: reversible, scoped to the task, confidence >= 0.8.
- `[WAIT]` — decision needed: lettered options (A/B/C), one marked `(recommended)`. Each option states what happened, what the choice does, and what the human must do.
- `[HUMAN]` — a step only the human can run; give the exact command or action.

Destructive, shared-state, low-confidence, or out-of-scope actions are never `[DONE]`.

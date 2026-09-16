# Failure tracking

Local failure log for the `harness-hook-simple-english` extension.
Kernel-side failures live in the kernel's own `docs/failure-tracking.md`.
The IDs below are scoped to this repo and restart at FT-001.

## FT-001 — Reply-gate refire: model reads its re-injected reply as user confirmation

**Symptom**
Session `rushi-tui/sessions/tui-highlight-nix`, events 1087 to 1101.
The agent's final report (1087) was gated by the `run.idle` reply gate.
The silent refire re-presented the agent's own reply to the model.
The model read it as a user confirmation.
The user sent no message in that window.

Event 1093: "Confirmed — all work is in place and verified."
Event 1099: "Understood — nothing to act on."
The real user input ("Commit.") arrived later at event 1101.
Two refires were consumed before the kernel cap of 2 stopped the loop.

**Root cause**
The silent refire re-shows the agent's own last reply.
The old `pending_feedback` read: "Your last reply was blocked by the writing rules. Revise that reply."
It named the writing rules but never said this block is not a user message.
It never said the user confirmed or approved nothing.
The model filled the gap with "the user is confirming."

**Fix**
`pending_feedback` now opens with an explicit hook-origin marker.
It states that no new user message arrived and the user confirmed nothing.
It says not to treat the block as a user instruction or a plan confirmation.
It scopes the edit to the flagged lines, keeping the same meaning.
The reply body is not re-injected. The violation list already names each issue with line and column.

**Verification**
`cargo test` passes 112/112.
The new test `run_idle_feedback_carries_hook_origin_marker` asserts the feedback:
opens with the `[writing-rules gate, not a user message]` marker,
contains the non-confirmation statement,
and does not contain the original reply body.

**Related (out of scope here)**
An optional kernel/TUI enhancement would label each refired reply as
"gated revision n" so a human reader sees the provenance.
That is a kernel-side change and is not part of this fix.

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

## FT-002 — Goal `run.idle` refire: model re-posts its own final reply

**Symptom**
Session `rushi/sessions/readme-writing`.
The model posted a final recommendation at 23:45:12 (log line 667).
That reply ended with a question to the user.
No new user message arrived in that window.

The model re-verified and re-posted the same recommendation at 23:45:27
and 23:45:41 (log lines 673 and 689).
Two silent refires fired between the first post and the cap.
To the human reader it looked like the user was echoing the model's own
reply.
The real user message arrived at 23:48:15 (log line 691).

**Root cause**
A `run.idle` hook returned `continue` with `refire`.
The kernel re-ran the model in place, with no new user message.
Each refired call re-presented the model's own last reply.
It also re-presented the original user turn (user_seq 540, the
v0.1.1 question).
The model read that re-presentation as "keep working."
It re-posted its own recommendation instead of waiting for the user.

The FT-001 hook-origin marker targets the writing-gate
`pending_feedback` path.
It does not cover a goal `run.idle` `continue`+`refire` that
re-injects the model's own reply.
So the marker was absent in this path.
The model did not know the re-presented text was its own output.

**Driver (from the log)**
The refire was driven by the goal continuation hook, not the writing
gate.
The `model_call_context` markers anchor each refired call to user_seq 540.
The `run.refire` and `run.refire_cap` markers confirm the silent-refire
path.
The cap is `[run] max_silent_refires` (default 2,
`docs/reference/README.md` section 6.4).

This session used the latest release build of the
`rushi-simple-english` hook.
The simple-english gate is a separate path from this goal refire.

**Evidence** (`~/programming/rust-unix-harness/sessions/readme-writing/events.jsonl`)

- line 667, 23:45:12: `assistant_message`, the recommendation
  ending in a question to the user.
- line 668, 23:45:12: `ext_status` id `run.refire`, value `{"n":2}`.
- lines 669-672: `loop_phase "wait"`, `hook.model.before "transform"`,
  `hook_applied` (goal-arm), `model_call_context {"user_seq":540}`.
- line 673, 23:45:27: `assistant_message`, "Let me verify two things
  before recommending…", with no user input.
- line 689, 23:45:41: `assistant_message`, the recommendation
  re-posted a second time.
- line 690, 23:45:41: `ext_status` id `run.refire_cap`,
  value `{"cap":2,"refires":2}`.
- line 691, 23:48:15: `user_message`, the real input. It names
  this confusion and says the FT-001 mitigation was not enough.

**Fix status**
Fix designed. A patch is prepared. The patch is not yet applied to the
goal-app repo. It waits for the owner of `rushi-exts` to review it.

**Fix (designed)**
Add a hook-origin marker to the goal continuation paths.
The change lives in `rushi-exts/goal-app`, not in this repo.
Two functions in `goal-state/src/lib.rs` carry the fix.

1. `build_continue_prompt()` now opens with the marker
   `[goal-continuation hook, not a user message]`.
   It states that no new user message arrived.
   It states that the user confirmed or approved nothing.
   This mirrors the FT-001 writing-gate marker.
2. `build_goal_fragment()` gains a static
   "Continuation provenance" note. The note is static text.
   It keeps the fragment byte-stable (P16/P17).
   The note covers the silent-refire case. In that case no message
   is logged. The model only re-sees its own reply.

**Patch**
The full patch is at `scratch/ft002-goal-hook.patch` in this repo.
It covers `goal-state/src/lib.rs` and the `hook-goal-idle` test.
Apply it inside a `rushi-exts/goal-app` checkout with `git apply`.
The patch adds three new tests in `goal-state`.
It adds one assertion to the `hook-goal-idle` test.
All 32 goal-state tests, 4 goal-idle tests, and 9 goal-arm tests
pass with the patch applied.

**Open item (cross-repo, pending)**
Apply the patch in `rushi-exts/goal-app`.
Then build `goal-state`, `hook-goal-idle`, and `hook-goal-arm`.
Then run `run-idle-continue-e2e.sh` in `rushi-exts`.
Note: that e2e script still uses the legacy `[paths] tools_root`
and `extra_tools_roots` keys. The current kernel renamed them to
`native_tool_paths` and `extension_tool_paths`.
Update the script before running the suite.

# Reply gate: silent refire (kernel issues #4 + #6)

## Decision

On 2026-09-16 the user confirmed that kernel issue #6 is implemented.
It landed as kernel commit `8a4bcfe`. The user asked that the
violation prompt route back to the agent with no user message at all.

The `run.idle` gate now emits `continue` with `log_message: false`
and `refire: true`. The kernel runs a model turn in place and logs
no user message at all. The correction prompt rides the hook's
`model.before` transform on that refired call. It lands as the last
user item of `request.input`. The fragment stays byte-stable, so the
cached prompt prefix holds (issue #5). The model revises the gated
reply in the same run, so no blank user panel appears in the TUI.
The row widget carries the status instead.

## Delivery path

1. `run.idle` lints the last assistant reply.
2. On hard violations, the hook stores the correction prompt in
   `<session>/simple-english-state.json` as `pending_feedback`.
   It also emits the silent refire continue.
3. The kernel runs a model turn in place. It logs a `run.refire`
   marker and no `user_message`. The `model.before` transform
   appends `pending_feedback` as the last user item of
   `request.input` on that refired call. It then clears it. The
   fragment stays byte-stable, so the prompt-cache prefix survives.
4. The revised reply is linted on the next `run.idle`. A clean
   reply settles the gate and turns the row green. A new violating
   reply gates and refires again.
5. The TUI row widget reads the same state file. It shows
   `n hard, m soft` until the reply is clean.

## Bounds and fallback

- The kernel caps the silent loop per run. The cap is `[run]
  max_silent_refires`, default 2. A value of 0 disables refires.
  A cap hit logs `run.refire_cap` and ends the run.
- The hook stops requesting gates after 3 consecutive gates.
  That is `MAX_GATE_COUNT`.
- When the cap hits, the pending feedback survives in the state
  file. The next model call delivers it.
- The same happens when the running kernel lacks `refire` or has
  it disabled. That is the issue #4 fallback.
- The run stays free of user messages on every path.

## Verification

- `cargo test` runs the gate unit tests. They assert `refire: true`
  on the gate decision. The test `run_idle_gate_chain_on_new_replies`
  checks the gate chain. It allows three refires, then stops at the
  gate-count limit.
- `scripts/refire-reply-gate-e2e.sh` runs the real hook against
  `rushi run` with a stub model.
  - `refire-recovers`: the violating reply is gated with one refire
    marker. The refired request carries the feedback as the last
    user item of `request.input`.
    The clean revision ends the run. The log holds one
    `user_message` (the seed). It holds zero empty ones.
  - `cap-fallback`: a model that never fixes the reply hits the
    default cap. Two `run.refire` markers and one `run.refire_cap`
    marker appear. The log still holds a single `user_message`.
    The pending feedback survives in the state file. It waits for
    the next model call.

## History

- Issue #4 (silent continue, `log_message: false`) landed first.
  The gate then waited for the user's next message to deliver the
  correction.
- 2026-09-16: kernel issue #6 shipped the `refire` flag. The gate
  switched to silent refire that same day.

## Follow-up (closed)

Filed as `TonyWu20/rushi#6`: let a silent continue re-fire the
model turn in the same run. It was implemented in the kernel on
2026-09-16. The commit is `8a4bcfe` (PR #7). The gate wired it in
the same day. Closed.

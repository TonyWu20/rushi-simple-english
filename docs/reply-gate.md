# Reply gate: logged follow-up

## Decision

On 2026-09-19 the user retired the silent refire mechanism. The
motivation was complexity reduction. The silent refire re-presented
the agent's own reply to the model. The local model read that as a
user echo. It invented user confirmations. The gate then gated the
invented content. Three failures were recorded: FT-001, FT-002, and
FT-003. Markers and role changes did not hold.

The gate now emits `continue` with a `message`. The kernel logs that
message as a `user_message` in the `follow` queue. The next step
drains it as a new model turn. The model sees a genuine user turn
asking it to revise the flagged lines. The transcript keeps a normal
user/assistant alternation.

## Delivery path

1. `run.idle` lints the last assistant reply.
2. On hard violations, the hook returns `continue` with a `message`.
   The message lists the hard and soft violations. It tells the
   model to revise only the flagged lines. It forbids re-posting the
   reply and answering its own open questions as user replies.
3. The kernel logs the message as a `user_message` with
   `queue = "follow"`. The next step injects it and runs a model
   turn.
4. The revised reply is linted on the next `run.idle`. A clean
   reply settles the gate and turns the row green. A new violating
   reply gates again, as a new logged follow-up.
5. The TUI row widget reads the same state file. It shows
   `n hard, m soft` until the reply is clean.

## What the hook no longer does

- It stores no `pending_feedback` in the state file. The state file
  holds `hard`, `soft`, `last_identity`, `gate_count`, and
  `gated_replies` only.
- The `model.before` transform injects the byte-stable rule summary
  fragment only. It touches `request.input` not at all. The cached
  prompt prefix survives.
- It emits no `refire` flag and no `log_message: false`.

## Bounds

- The hook stops gating after 3 consecutive gates. That is
  `MAX_GATE_COUNT`. The run stops with the last violating reply
  standing.
- Each gate is one logged user message plus one model call. The TUI
  shows the follow-up as a user panel. That is the accepted cost of
  retiring the silent mechanism.

## Verification

- `cargo test` runs the gate unit tests. They assert a `message`
  with no `refire` flag on the gate decision. The test
  `run_idle_gate_chain_on_new_replies` checks the gate chain. It
  allows three logged follow-ups, then stops at the gate-count
  limit.
- `scripts/reply-gate-e2e.sh` runs the real hook against `rushi
  run` with a stub model.
  - `gate-recovers`: the violating reply is gated with one logged
    follow-up. The follow-up request carries the gate text as the
    last user item of `request.input`. The clean revision ends the
    run. The log holds two `user_message` events (the seed and one
    gate follow-up). It holds zero empty ones. No `run.refire`
    marker appears.
  - `gate-caps`: a model that never fixes the reply produces three
    logged follow-ups. The run stops at `MAX_GATE_COUNT`. The
    counter stops at 3. No `run.refire` marker appears.

## History

- Issue #4 (silent continue, `log_message: false`) landed first.
  The gate then waited for the user's next message to deliver the
  correction.
- 2026-09-16: kernel issue #6 shipped the `refire` flag. The gate
  switched to silent refire that same day.
- 2026-09-19: the user retired the silent refire. The kernel
  dropped the `refire` payload flag, the `run.refire` and
  `run.refire_cap` markers, and `[run] max_silent_refires`. The gate
  switched to a logged follow-up message. The kernel mechanism is
  gone; this is the only continuation path.

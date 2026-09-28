# Reply gate: logged follow-up

## Decision

On 2026-09-19 the user retired the silent refire mechanism. The
motivation was complexity reduction. The silent refire re-presented
the agent's own reply to the model. The local model read that as a
user echo. It invented user confirmations. The gate then gated the
invented content. Three failures were recorded: FT-001, FT-002, and
FT-003. Markers and role changes did not hold.

The gate now appends its own follow-up `user_message` to the session
log via the `LOG_BIN` binary (the §12 pipeline ABI, issue #6). The
message rides the `follow` queue. The loop drains it as a new model
turn. The model sees a genuine user turn asking it to re-send the
complete, clean reply. The transcript keeps a normal user/assistant
alternation.

## Delivery path

1. `run.idle` lints the last assistant reply.
2. On hard violations (and the gate cap is not reached), the hook
   appends a `user_message` event to the session log via
   `"$LOG_BIN" --session "$SESSION"`. The event carries
   `queue: "follow"`, an RFC 3339 `ts`, and the gate text as
   `content`. The gate text lists the hard and soft violations and
   tells the model to re-send the complete reply, in full, with
   every flagged issue fixed, keeping the meaning. It forbids
   re-verifying the work and answering its own open questions as
   user replies.
3. The hook prints `{}` and exits 0. The loop drains the pending
   follow-up `user_message` and runs a model turn.
4. An append failure (e.g. `LOG_BIN` missing) is a step-level
   failure: the hook exits 3, the kernel logs
   `hook.run.idle.error`, the window resolves to its default
   (stop), and the loop never wedges.
5. The revised reply is linted on the next `run.idle`. A clean
   reply settles the gate and turns the row green. A new violating
   reply gates again, as a new logged follow-up.
6. The TUI row widget reads the same state file. It shows
   `n hard, m soft` until the reply is clean.

## What the hook no longer does

- It stores no `pending_feedback` in the state file. The state file
  holds `hard`, `soft`, `last_identity`, `gate_count`, and
  `gated_replies` only.
- The `model.before` transform injects the byte-stable rule summary
  fragment only. It touches `request.input` not at all. The cached
  prompt prefix survives.
- It emits no `refire` flag, no `log_message: false`, and no §4.3
  decision envelope.

## Bounds

- The hook stops gating after 3 consecutive gates. That is
  `MAX_GATE_COUNT`. The run stops with the last violating reply
  standing.
- Each gate is one logged user message plus one model call. The TUI
  shows the follow-up as a user panel. That is the accepted cost of
  retiring the silent mechanism.

## Verification

- `cargo test` runs the gate unit tests. They assert the gate
  message content and the `Option<String>` return from
  `run_idle_decision` (some = append, none = clean/capped). The test
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
  switched to a logged follow-up message. That makes the logged
  follow-up the only continuation path.
- 2026-09-20: the user found the "revise only the flagged lines"
  wording frustrating. The run ended on a patch of changed lines.
  The user could not read a complete clean reply. The gate now asks
  the model to re-send the whole reply, in full, with the flagged
  issues fixed. See FT-004.
- 2026-09-28: §12 pipeline ABI migration (issue #6). The hook now
  appends its own follow-up `user_message` via `LOG_BIN` instead of
  emitting a `{"decision":"continue","payload":{"message":...}}`
  envelope for the kernel to log. Dispatch moved from the payload
  `window` key to the `HARNESS_WINDOW` env var. `tool.before` emits
  `{"blocked_calls":[{id,reason}]}` instead of a §4.3 block
  envelope. `model.before` treats the stdin state as the request
  object and emits the transformed request directly.
- 2026-09-28: user decision — the gate's RFC 3339 timestamp is
  generated with `chrono` (`Utc::now()`, second-precision UTC),
  superseding the issue's original "pure-std helper, no chrono"
  requirement. `chrono` is now a dependency of the hook (the kernel
  already formats its event timestamps with chrono).

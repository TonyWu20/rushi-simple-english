#!/usr/bin/env bash
# e2e for the reply-gate silent refire (kernel issue #6): the real
# harness-hook-simple-english gates a violating reply, the kernel
# refires the model turn in place with NO user message logged, the
# hook's model.before transform delivers the correction prompt to
# that refired call, and the model's clean revision ends the run.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cargo build --quiet --release || {
  echo "FAIL cargo build --release"
  exit 1
}
HOOK_BIN="$ROOT/target/release/harness-hook-simple-english"
[[ -x "$HOOK_BIN" ]] || { echo "FAIL hook binary missing"; exit 1; }

# The kernel binary the loop runs with (a sibling checkout is
# expected; the TUI config points there too).
K="$(cd "$ROOT/../rust-unix-harness" 2>/dev/null && pwd)"
RUSHI_BIN=""
for c in "$K/target/release/rushi" "$K/target/debug/rushi"; do
  [[ -x "$c" ]] && { RUSHI_BIN="$c"; break; }
done
[[ -n "$RUSHI_BIN" ]] || {
  echo "FAIL no rushi kernel binary at $K/target/{release,debug}"
  exit 1
}

PASS=0
FAIL=0
ok() {
  PASS=$((PASS + 1))
}
ko() {
  FAIL=$((FAIL + 1))
  echo "FAIL $1"
}

work_config() {
  cat > "$WORK/config.toml" <<EOF
[model]
api = "responses"
max_output_tokens = 4096

[model.stub]
model_id = "stub-model"
base_url = "http://127.0.0.1:1"
api_key_env = "DUMMY"
context_tokens = 8000

[active]
model = "stub"

[paths]
sessions_root = "sessions"

[limits]
context_budget_tokens = 8000
compact_reserve_tokens = 500
compact_keep_tokens = 2000
compact_enabled = false

[system_prompt]
text = "test"

[hooks]
timeout_ms = 30000

[[hooks.on]]
window  = "model.before"
command = "$HOOK_BIN"
args    = []

[[hooks.on]]
window  = "run.idle"
command = "$HOOK_BIN"
args    = []
${1:-}
EOF
}

seed_session() {
  cat > "$SLOG" <<EOF
{"v":1,"type":"user_message","ts":"t1","seq":1,"content":"do the task"}
EOF
}

# The model: its first reply breaches the writing rules (seven
# sentences in one paragraph), its second reply is clean.
make_stub() {
  printf '%s\n' '#!/usr/bin/env bash' > "$WORK/stub-model"
  cat >> "$WORK/stub-model" <<'EOF'
set -u

if [[ "${1:-}" == "--describe" ]]; then
  jq -cn '{active: "stub", model_id: "stub-model", reasoning_effort: "none", thinking_level: "off"}'
  exit 0
fi

req=$(cat)
printf '%s\n' "$req" >>"${STUB_REQLOG:-/dev/null}"
n=$(wc -l <"${STUB_REQLOG}" | tr -d ' ')

if [[ "$n" -eq 1 ]]; then
  jq -cn '{text: "One. Two. Three. Four. Five. Six. Seven.", tool_calls: [], reasoning: [], stop_reason: "stop", usage: {input_tokens: 10, output_tokens: 10}}'
else
  jq -cn '{text: "The task is complete.", tool_calls: [], reasoning: [], stop_reason: "stop", usage: {input_tokens: 10, output_tokens: 10}}'
fi
EOF
  chmod +x "$WORK/stub-model"
}

# The model keeps answering with a violating reply (a fresh one each
# call, so the gate chain keeps going): proves the cap bounds the
# silent loop and the feedback survives as the fallback.
make_stub_violating() {
  printf '%s\n' '#!/usr/bin/env bash' > "$WORK/stub-model"
  cat >> "$WORK/stub-model" <<'EOF'
set -u

if [[ "${1:-}" == "--describe" ]]; then
  jq -cn '{active: "stub", model_id: "stub-model", reasoning_effort: "none", thinking_level: "off"}'
  exit 0
fi

req=$(cat)
printf '%s\n' "$req" >>"${STUB_REQLOG:-/dev/null}"
n=$(wc -l <"${STUB_REQLOG}" | tr -d ' ')
jq -cn --arg t "One. Two. Three. Four. Five. Six. Seven. (pass $n)" \
  '{text: $t, tool_calls: [], reasoning: [], stop_reason: "stop", usage: {input_tokens: 10, output_tokens: 10}}'
EOF
  chmod +x "$WORK/stub-model"
}

run_loop() {
  (
    cd "$WORK"
    export CONFIG="$WORK/config.toml"
    export MODEL_BIN="$WORK/stub-model"
    export STUB_REQLOG="$WORK/reqlog"
    : >"$STUB_REQLOG"
    timeout 120 "$RUSHI_BIN" run session
  ) >/dev/null 2>&1
  true
}

user_message_count() {
  jq -c 'select(.type == "user_message")' "$SLOG" 2>/dev/null | wc -l | tr -d ' '
}

empty_user_message_count() {
  jq -c 'select(.type == "user_message" and .content == "")' "$SLOG" 2>/dev/null | wc -l | tr -d ' '
}

count_markers() {
  local n
  n=$(jq -c "select(.type == \"ext_status\" and .id == \"$1\")" "$SLOG" 2>/dev/null | wc -l)
  echo "$n"
}

req_line_has() {
  # $1: line number of the reqlog, $2: needle
  sed -n "$1p" "$STUB_REQLOG" 2>/dev/null | grep -qF "$2"
}

state_field() {
  jq -r ".$1" "$SESSIONS_DIR/simple-english-state.json" 2>/dev/null
}

assert_eq() {
  if [ "$1" = "$2" ]; then
    ok
  else
    ko "$3: got [$1], want [$2]"
  fi
}

NEW_WORK() {
  WORK="$ROOT/scratch/e2e-refire-reply-gate/$1"
  rm -rf "$WORK"
  SESSIONS_DIR="$WORK/sessions/session"
  SLOG="$SESSIONS_DIR/events.jsonl"
  STUB_REQLOG="$WORK/reqlog"
  mkdir -p "$SESSIONS_DIR"
  echo "==== scenario: $1"
}

# A violating reply is gated, the kernel refires the model in place,
# the refired request carries the correction prompt, and the clean
# revision ends the run: two model turns, one user_message (the
# seed), no follow user message, no blank user message.
scenario_refire_recovers() {
  NEW_WORK refire-recovers
  work_config ""
  seed_session
  make_stub
  run_loop
  assert_eq "$(user_message_count)" "1" \
    "only the seed user_message, no follow appended"
  assert_eq "$(empty_user_message_count)" "0" \
    "no blank user_message in the log"
  assert_eq "$(wc -l < "$STUB_REQLOG" | tr -d ' ')" "2" \
    "the seed turn plus one silent refire turn"
  assert_eq "$(count_markers "run.refire")" "1" \
    "one refire marker"
  assert_eq "$(count_markers "run.refire_cap")" "0" \
    "the cap was not reached"
  if req_line_has 2 "Pending reply feedback"; then
    ok
  else
    ko "the refired request carries the correction prompt"
  fi
  if req_line_has 1 "Pending reply feedback"; then
    ko "the first request must not carry the feedback"
  else
    ok
  fi
  assert_eq "$(state_field hard)" "0" \
    "the clean revision settles the gate"
  assert_eq "$(state_field pending_feedback)" "null" \
    "the feedback was consumed by the refired call"
}

# The gate keeps refiring on a model that never fixes the reply:
# the per-run cap (default 2) bounds the silent loop, the run stops
# with a cap marker, and the last pending feedback survives in the
# state file for delivery on the next model call (issue #4 fallback).
scenario_cap_fallback() {
  NEW_WORK cap-fallback
  work_config ""
  seed_session
  make_stub_violating
  run_loop
  assert_eq "$(user_message_count)" "1" \
    "no user_message appended by refires"
  assert_eq "$(wc -l < "$STUB_REQLOG" | tr -d ' ')" "3" \
    "the seed turn plus the two allowed refires"
  assert_eq "$(count_markers "run.refire")" "2" \
    "two refire markers under the default cap"
  assert_eq "$(count_markers "run.refire_cap")" "1" \
    "the cap marker was logged"
  if [[ "$(state_field pending_feedback)" == *"Hard violations"* ]]; then
    ok
  else
    ko "the pending feedback survives for the next model call"
  fi
}

scenario_refire_recovers
scenario_cap_fallback

echo
echo "refire-reply-gate-e2e: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]

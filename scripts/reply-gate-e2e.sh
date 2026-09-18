#!/usr/bin/env bash
# e2e for the reply-gate logged follow-up: the real
# harness-hook-simple-english lints the last assistant reply, and on a
# hard violation returns `continue` with a `message`. The kernel logs
# that message as a follow `user_message` and drains it as a new model
# turn. The model revises the reply within the same run. The silent
# refire mechanism is retired; no `refire` flag, no `run.refire`
# marker.

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
# call, so the gate chain keeps going): proves MAX_GATE_COUNT bounds
# the logged follow-ups.
make_stub_violating() {
  printf '%s\n' '#!/usr/bin/env bash' > "$WORK/stub-model"
  cat >> "$WORK/stub-model" <<'EOF'
set -u

if [[ "${1:-}" == "--describe" ]]; then
  jq -cn '{active: "stub", model_id: "stub-model", reasoning_effort: "none", thinking_level: "off"}'
  exit 0
fi

req=$(cat)
printf '%s\n' "$req" >>"${STUB_REQLOG}"
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

gate_message_count() {
  # The logged follow-up user messages carry the gate text.
  jq -c 'select(.type == "user_message" and ((.content // "") | contains("writing-rule violation")))' "$SLOG" 2>/dev/null | wc -l | tr -d ' '
}

count_markers() {
  local n
  n=$(jq -c "select(.type == \"ext_status\" and .id == \"$1\")" "$SLOG" 2>/dev/null | wc -l)
  echo "$n"
}

req_tail_has_gate_text() {
  # $1: line number of the reqlog. True when the last input item is a
  # user message carrying the gate follow-up text.
  sed -n "$1p" "$STUB_REQLOG" 2>/dev/null | jq -e '
    (.input | type) == "array"
    and (.input[-1].type == "message")
    and (.input[-1].role == "user")
    and ((.input[-1].content // "") | contains("writing-rule violation"))
  ' >/dev/null 2>&1
}

req_instructions_carry_gate_text() {
  # $1: line number of the reqlog. True when instructions carry the
  # gate text. The gate text must ride the logged user message, never
  # the prompt head.
  sed -n "$1p" "$STUB_REQLOG" 2>/dev/null | jq -e '
    (.instructions // "") | contains("writing-rule violation")
  ' >/dev/null 2>&1
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
  WORK="$ROOT/scratch/e2e-reply-gate/$1"
  rm -rf "$WORK"
  SESSIONS_DIR="$WORK/sessions/session"
  SLOG="$SESSIONS_DIR/events.jsonl"
  STUB_REQLOG="$WORK/reqlog"
  mkdir -p "$SESSIONS_DIR"
  echo "==== scenario: $1"
}

# A violating reply is gated, the kernel logs the follow-up and drains
# it as a new model turn, and the clean revision ends the run: two
# model turns, two user_messages (the seed plus one gate follow-up),
# no refire marker.
scenario_gate_recovers() {
  NEW_WORK gate-recovers
  work_config ""
  seed_session
  make_stub
  run_loop
  assert_eq "$(user_message_count)" "2" \
    "the seed plus one gate follow-up user_message"
  assert_eq "$(empty_user_message_count)" "0" \
    "no blank user_message in the log"
  assert_eq "$(gate_message_count)" "1" \
    "exactly one logged gate follow-up"
  assert_eq "$(wc -l < "$STUB_REQLOG" | tr -d ' ')" "2" \
    "the seed turn plus one follow-up turn"
  assert_eq "$(count_markers "run.refire")" "0" \
    "the refire mechanism is retired"
  assert_eq "$(count_markers "run.refire_cap")" "0" \
    "no refire cap marker"
  # The follow-up request carries the gate text as the last user item
  # of the input, and the seed request tail must not carry it.
  if req_tail_has_gate_text 2; then
    ok
  else
    ko "the follow-up request tail is the gate user item"
  fi
  if req_tail_has_gate_text 1; then
    ko "the seed request tail must not carry the gate text"
  else
    ok
  fi
  if req_instructions_carry_gate_text 1 || req_instructions_carry_gate_text 2; then
    ko "instructions must not carry the gate text"
  else
    ok
  fi
  assert_eq "$(state_field hard)" "0" \
    "the clean revision settles the gate"
  assert_eq "$(state_field gate_count)" "0" \
    "the clean revision resets the gate counter"
}

# The gate keeps logging follow-ups on a model that never fixes the
# reply: MAX_GATE_COUNT (3) bounds the logged follow-ups, the run
# stops with the last violating reply standing, and no refire marker
# is logged.
scenario_gate_caps() {
  NEW_WORK gate-caps
  work_config ""
  seed_session
  make_stub_violating
  run_loop
  assert_eq "$(user_message_count)" "4" \
    "the seed plus three gate follow-ups"
  assert_eq "$(gate_message_count)" "3" \
    "three logged gate follow-ups under MAX_GATE_COUNT"
  assert_eq "$(wc -l < "$STUB_REQLOG" | tr -d ' ')" "4" \
    "the seed turn plus three follow-up turns"
  assert_eq "$(count_markers "run.refire")" "0" \
    "the refire mechanism is retired"
  assert_eq "$(count_markers "run.refire_cap")" "0" \
    "no refire cap marker"
  assert_eq "$(state_field gate_count)" "3" \
    "the counter stopped at MAX_GATE_COUNT"
}

scenario_gate_recovers
scenario_gate_caps

echo
echo "reply-gate-e2e: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]

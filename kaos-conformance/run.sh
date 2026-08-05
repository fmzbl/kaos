#!/usr/bin/env bash
#
# Run every conformance program through the real `kaos` binary against a local
# model, and report what each one did.
#
#   ./kaos-conformance/run.sh                 # every program
#   ./kaos-conformance/run.sh 03 07           # only these
#   MODEL=llama3.2:3b ./kaos-conformance/run.sh
#
# This drives the SHIPPED BINARY, not a library harness. That is the point:
# what is being tested is the whole path a person uses — argument handling, the
# host's oracle and inlet, the provider, the record — and a test that linked the
# library directly would prove the parts while skipping the assembly.
#
# Each program carries its own expectations in a `; expect:` header, so the
# program and the thing it promises live in one file and cannot drift apart.
# See EXPECTATIONS below for the vocabulary.

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
kaos="$root/target/release/kaos"
# llama3.2:3b by default, and the choice is measured rather than a preference.
# On this machine qwen3:4b takes ~110s for a one-word answer even with
# reasoning off, so a fifty-program suite is an hour and a half of waiting and
# nobody runs it twice. The 3b answers the same prompt in ~10s. Conformance is
# about the LANGUAGE, and a language property that only holds on a slower model
# is not a language property — so the suite defaults to what finishes, and any
# model can be asked for.
model="${MODEL:-ollama:llama3.2:3b}"
timeout_s="${TIMEOUT:-120}"
out="$here/results"

# ── EXPECTATIONS ────────────────────────────────────────────────────────────
#
# A program declares what its run must show, one claim per header line:
#
#   ; expect: calls N          exactly N model calls
#   ; expect: calls >=N        at least N
#   ; expect: contains TEXT    TEXT appears somewhere in the run's output
#   ; expect: absent TEXT      TEXT appears NOWHERE — the check that catches a
#                              branch that should not have run
#   ; expect: clean            no diagnostics at all
#   ; expect: diagnostic TEXT  a diagnostic mentioning TEXT (an expected one)
#   ; expect: answers          the program produced a final result
#
# And one marker that is not an expectation:
#
#   ; requires: generation      this program asks a MODEL to write Rebis.
#                               Skipped unless GENERATION=1 or the program is
#                               named explicitly, because whether a model can
#                               write Rebis is a capability, not a property of
#                               the language.
#
# Counting calls is how a text-generating model is held to a structural claim:
# it cannot fake having been called twice.

if [[ ! -x "$kaos" ]]; then
  echo "no kaos binary at $kaos"
  echo "build it first:  cargo build --release"
  exit 2
fi

if ! curl -s --max-time 3 "${OLLAMA_HOST:-http://127.0.0.1:11434}/api/tags" >/dev/null; then
  echo "ollama is not answering at ${OLLAMA_HOST:-http://127.0.0.1:11434}"
  echo "start it first:  ollama serve"
  exit 2
fi

mkdir -p "$out"
selected=("$@")

passed=0
failed=0
declare -a failures=()

for program in "$here"/programs/*.rebis; do
  name="$(basename "$program" .rebis)"

  if [[ ${#selected[@]} -gt 0 ]]; then
    match=0
    for want in "${selected[@]}"; do
      [[ "$name" == *"$want"* ]] && match=1
    done
    [[ $match -eq 1 ]] || continue
  fi

  # A program marked `; requires: generation` asks a MODEL to write Rebis, and
  # that is a capability rather than a language property — the default 3B
  # cannot do it and a capable model can. Skipped unless asked for, so the
  # suite stays green on the default model without pretending the gap is not
  # there. Run them with GENERATION=1, and see the README.
  if grep -q '^; requires: generation' "$program" && [[ -z "${GENERATION:-}" ]]; then
    if [[ ${#selected[@]} -eq 0 ]]; then
      printf '%-26s skip  (needs a model that writes Rebis; GENERATION=1)\n' "$name"
      continue
    fi
  fi

  log="$out/$name.log"
  printf '%-26s ' "$name"

  # Run from the crate root so a program's own relative paths resolve, and with
  # a per-run dream so one program's memory cannot leak into another's.
  ( cd "$here" \
    && KAOS_MODEL="$model" \
       timeout "$timeout_s" "$kaos" rebis run "$program" ) >"$log" 2>&1
  status=$?

  if [[ $status -eq 124 ]]; then
    echo "TIMEOUT after ${timeout_s}s"
    failures+=("$name: timed out")
    failed=$((failed + 1))
    continue
  fi

  # `model    generating turn` is printed once per model call by the host, so
  # counting those lines counts the calls without instrumenting anything.
  calls=$(grep -c "^model    generating turn" "$log")
  problems=()

  while IFS= read -r claim; do
    claim="${claim#*; expect: }"
    kind="${claim%% *}"
    value="${claim#* }"
    case "$kind" in
      calls)
        if [[ "$value" == ">="* ]]; then
          want="${value#>=}"
          (( calls >= want )) || problems+=("wanted >=$want calls, made $calls")
        else
          (( calls == value )) || problems+=("wanted $value calls, made $calls")
        fi
        ;;
      contains)
        grep -qiF -- "$value" "$log" || problems+=("never said '$value'")
        ;;
      absent)
        grep -qiF -- "$value" "$log" && problems+=("said '$value', which should not have run")
        ;;
      clean)
        grep -q "^diagnostic" "$log" \
          && problems+=("diagnostics: $(grep '^diagnostic' "$log" | head -3 | tr '\n' ';')")
        ;;
      diagnostic)
        grep -q "^diagnostic.*$value" "$log" || problems+=("no diagnostic mentioning '$value'")
        ;;
      answers)
        grep -q "^result   " "$log" || problems+=("produced no result")
        grep -q "^result   nothing$" "$log" && problems+=("declined to answer")
        ;;
      *) problems+=("unknown expectation '$kind'") ;;
    esac
  done < <(grep '^; expect: ' "$program")

  if [[ ${#problems[@]} -eq 0 ]]; then
    echo "ok  ($calls calls)"
    passed=$((passed + 1))
  else
    echo "FAIL  ($calls calls)"
    for problem in "${problems[@]}"; do
      echo "    · $problem"
      failures+=("$name: $problem")
    done
    failed=$((failed + 1))
  fi
done

echo
echo "$passed passed, $failed failed · logs in $out"
if [[ ${#failures[@]} -gt 0 ]]; then
  echo
  echo "failures:"
  printf '  %s\n' "${failures[@]}"
  exit 1
fi

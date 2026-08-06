# Kaos harness vs opencode

A head-to-head on one model, one task set, one gate. The point is to measure the
**harness** — the agent loop, the action protocol, the context discipline — not
the model, so both sides drive the same `qwen3.6:35b` on the same Ollama box.

## Running it

```sh
python3 run_bench.py math   # 10 arithmetic/counting problems, exact-answer gate
python3 run_bench.py prog   #  5 coding tasks, pytest gate
python3 run_bench.py        # both
```

Results land in `results/results-<suite>.json` and a summary is printed.
`BENCH_TIMEOUT` (default 900s) bounds one task.

## What makes it a fair fight

- **Same model, same server.** `KAOS_MODEL=ollama:qwen3.6:35b` for Kaos;
  `opencode.json` here points opencode at the same host through its
  OpenAI-compatible `/v1` endpoint. That config is project-local on purpose —
  nothing in `~/.config/opencode` is touched.
- **Thinking is off for both.** Kaos can set Ollama's `think` flag; opencode
  reaches the model through `/v1`, which has no way to ask for it. Leaving it on
  would measure the flag rather than the harness, so the runner exports
  `KAOS_THINK=0`.
- **Neither harness grades itself.** Each coding task runs in a throwaway copy
  of its directory, and the runner copies a **pristine** `test_sol.py` back over
  whatever the agent left before running pytest. An agent that "passes" by
  editing the test scores zero.
- **Sessions are redirected.** `KAOS_SESSION_DIR` points at `results/sessions`,
  so a benchmark never files hundreds of turns into `~/.kaos/sessions`.

## Validity of the task set

Both properties are checked, and both matter:

- every math answer is **computed independently** in Python, not asserted from
  memory (one of the ten was wrong on the first pass and was caught this way);
- every coding gate **passes** against a reference solution and **fails**
  against the shipped starting state — so a do-nothing agent cannot score, and a
  correct agent is not blocked by a broken test.

## Reading a result

`ok` is the only thing that counts as a pass. `secs` is wall time for the whole
task including model load — note that Ollama reloads the model whenever
`num_ctx` changes, which costs ~65s, so the first task of a run is slower than
the rest by roughly that much.

## Reading results honestly

**One sample per task is noisy.** Sampling runs at the provider default
temperature, and consecutive runs of the same suite disagreed: `m02` failed in
one run and passed in the next, `m04` did the reverse. A gap of one or two tasks
between harnesses on a 10-task suite is **not** a result. Treat only a large,
repeated gap as signal, and re-run before concluding anything.

**Wall time is confounded by model loading.** Ollama reloads the model when
`num_ctx` changes, at ~65s a time. Kaos names a window (32768); opencode reaches
the model through `/v1` and does not, so it inherits Ollama's 4096. Switching
between the two harnesses therefore forces a reload, which is why the runner
does all of one harness before starting the other, and why the first task of
each block is ~100s slower than the rest. Compare `secs` within a harness, not
across.

That difference is itself a finding rather than a nuisance: a 4096-token window
is smaller than a multi-turn agent transcript, so the harness that never names
its window is the one that will silently truncate first.

## Calibrating the instrument

```sh
python3 selftest.py     # no model, no network
```

Checks the three properties a run depends on: every stated answer is recomputed
from scratch, a failed call is never mined for digits, and the coding gate holds
against an agent that solves, an agent that does nothing, and an agent that
rewrites the test. It has already caught a wrong answer key twice — once in the
problem set, once in its own checker — so run it before trusting a result.

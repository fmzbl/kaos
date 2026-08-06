# Findings — Kaos harness vs opencode, qwen3.6:35b

Status: **incomplete — blocked on hardware.** The model host
(192.168.10.197) went off the network part-way through the first full run and
has not returned. The fault is on that box, not on the machine running the
benchmark: the default gateway answers and the WiFi link is up, while the host
answers neither ICMP nor HTTP. A host-aware run is parked with a 12-hour window
and starts itself the moment the box is back.

No substitute was used. The only models on localhost are `qwen3:4b` and
`llama3.2:3b`; swapping either in would answer a different question than the one
asked, and a benchmark run against the wrong model is worse than no benchmark.

What follows is only what the evidence actually supports.

## Suite

20 math problems and 8 coding tasks — 56 runs, 28 per harness. It started at
10 and 5 and was doubled during the outage, because the noise turned out to be
large enough that the smaller set could not separate a real gap from sampling
(see the caveats below).

## What was measured

Kaos completed 7 of the first 10 math problems before the host dropped:

| | |
|---|---|
| kaos, problems that ran | **6 / 7** |
| kaos, excluded (host down) | 3 |
| opencode | **no valid data** |

The single genuine failure was `m04`: the model answered `312` where the answer
is `624` — it missed "twice a week". The raw capture shows a clean bare-integer
reply, so neither the harness nor the grader is implicated. That is a model
error.

**opencode has no score.** All of its tasks ran after the host died and timed
out at 900s each. Reporting `0/10` for opencode would be false — it never got
to attempt anything.

## Bugs this benchmark found in itself

Worth recording, because each one would have produced a confident wrong number.

1. **A wrong answer key.** One of the ten stated answers was wrong on the first
   pass. Every answer is now computed independently in Python and checked
   against the stated one before any run.

2. **The grader scored error text.** With the host unreachable, the reply was
   `…http://192.168.10.197:11434/api/generate: …no route to host…`, and a
   "last integer in the output" grader returned **11434** — the port — as the
   model's answer. Two more problems scored `113`, the errno. Replies carrying a
   failure mark are now excluded rather than mined for digits.

3. **A failed run looked like a bad score.** opencode does not fail fast, so a
   dead host produced ten rows indistinguishable from ten wrong answers. The
   runner now checks the host before each task, waits for it to return, and
   aborts rather than filling a results file with fiction.

4. **A trap that would have benchmarked the wrong model.** `kaos code` falls
   back to the Claude CLI whenever the bound model is `sim`
   (`src/main.rs`: *"A simulated session has none — default to the claude
   CLI"*). A misconfigured run would have reported Claude's programming scores
   under Kaos's name with nothing in the output saying so. A preflight now
   refuses to start unless the model is an ollama model.

## Caveats that will still apply to the finished run

- **One sample per task is noisy.** Consecutive runs disagreed: `m02` failed in
  one and passed in the next; `m04` did the reverse. A gap of one or two tasks
  on a 10-task suite is not a result.
- **Wall time is not comparable across harnesses.** Ollama reloads the model
  when `num_ctx` changes (~65s, measured). Kaos names a window; opencode reaches
  the model through `/v1` and inherits Ollama's 4096 default. Compare `secs`
  within a harness only.
- That window difference is itself a real asymmetry: 4096 tokens is smaller
  than a multi-turn agent transcript, so the harness that never names its window
  is the one that truncates first. It is a fair thing to measure, but it is a
  harness *configuration* difference, not a reasoning difference.

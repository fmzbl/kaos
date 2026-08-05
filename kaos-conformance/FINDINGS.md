# Findings

What building and running the conformance suite turned up. Kept as a file
rather than a message because the next person to run this will hit the same
things, and two of them cost an afternoon each.

## 1. Every Rebis node call carried the whole language reference · **fixed**

`RebisOracle` builds its system prompt with
`kaos_agent::conductor::rebis_agent_system_prompt()`, which embeds
`REBIS_AUTHORING_CONTEXT` — the entire contents of `docs/REBIS_CHAT_CONTEXT.md`,
**~18 KB, roughly 4,500 tokens**. On the ollama path
(`provider.rs:253`) system and user are concatenated into one prompt, so every
node of every program pays those tokens before its own words.

On a hosted model this is invisible: it caches, and the latency is dominated by
the network. On a small local model on CPU it is the whole cost — a one-word
answer takes minutes, and a fifty-program suite never finishes.

The content is also wrong for the job. The authoring context teaches a model how
to *write* Rebis. A node executing `"Answer with exactly the word: alpha"` is not
authoring anything, and the prompt itself says so two paragraphs later: *"You are
now executing one node, not authoring the surrounding program."* We are sending
4,500 tokens of instructions and then telling the model to ignore them.

**Fixed** in `rebis_agent_system_prompt`: a node now gets its contract and
nothing else, and the reference stays on the chat path where a model actually
authors Rebis. The node prompt went from ~18 KB to under 600 bytes.

**Honesty about the evidence.** This was found by reading the code while
diagnosing a timeout, and it is a genuine defect on its own terms — 4,500
tokens sent per node in order to be countermanded two sentences later. But the
timeout it was found while chasing was *not* caused by it: a control
measurement afterwards showed a bare `say hi`, with no system prompt at all,
also timing out at 200 s. See #2. The fix is right; the diagnosis that led to
it was luck.

## 2. A saturated ollama server looks exactly like a host bug · **environmental**

This is the one that actually blocked the run, and it is worth describing
precisely because it wasted most of a session.

A model interrupted mid-generation leaves its runner spinning — measured here at
**594 % CPU with a load average of 9.7** — and every later request queues behind
it. Worse, each timed-out request *adds* to the queue, so investigating the
symptom makes the symptom worse. The failure mode is a timeout with no error,
which is indistinguishable from a hang in the host.

The tell: Kaos holds **zero sockets and zero child processes** while apparently
hung. If it were Kaos, there would be a connection to look at.

```bash
ps -o pid,pcpu,etime -p "$(pgrep -f 'ollama runner' | head -1)"
sudo systemctl restart ollama     # kill -9 will not work: not your user
```

Not a Kaos bug, but it cost hours to distinguish from one, so it is in the
README.

## 3. Model load time is minutes, not seconds · **environmental**

The first call to a model pays load time: measured at **~110 s for qwen3:4b** and
**~30 s for llama3.2:3b** on this machine, against ~1 s once warm. A suite that
starts cold looks broken.

Warm the model before running, and the first program is not an outlier:

```bash
curl -s http://127.0.0.1:11434/api/generate \
  -d '{"model":"llama3.2:3b","prompt":"hi","stream":false}' >/dev/null
```

## 4. qwen3:4b is not a usable conformance model here · **judgement**

Even with `think:false`, it takes ~110 s for a one-word answer on this CPU. The
suite defaults to `llama3.2:3b` instead. A conformance suite proves the
*language*, and a language property that only holds on a slower model is not a
language property — but a suite nobody runs proves nothing at all.

`MODEL=ollama:qwen3:4b ./run.sh` still works, for when the wait is affordable.

## 5. Doc examples are executed, and a missing file failed them · **fixed**

`tests/doc_examples.rs` runs every Rebis block in the README. Adding a
`(&: "./bug.png")` example broke it, because the reader does not have that file.

Correct behaviour from the checker — it already had the same allowance for a
missing *module* — so the fix was to extend it: `InputUnavailable`, and the
`UnboundValue` cascade it causes, are expected. An example showing the shape of
reading a file should not require the reader to have that file.

## What this suite has NOT yet proved

Being exact, because the difference matters and it would be easy to imply
otherwise.

**Nothing here has been confirmed against a live model.** The machine's ollama
server was saturated for the whole session (#2), so no program completed a run.
Every `.rebis` program parses, and the runner and its expectation vocabulary
work — but the column that says `ok (2 calls)` has not been seen for a single
program yet.

What *is* proved, by 273 rebis tests and 712 Kaos tests, is the runtime under a
scripted oracle: every operator's call count, transport, laziness, and
diagnostics. That is not nothing, and it is also not what this suite is for.
The gap between those two is exactly where finding #1 lived — visible only when
a real request is assembled — which is the argument for running this the moment
the machine is free.

**To finish it:**

```bash
sudo systemctl restart ollama
curl -s http://127.0.0.1:11434/api/generate \
  -d '{"model":"llama3.2:3b","prompt":"hi","stream":false}' >/dev/null   # warm it
./kaos-conformance/run.sh
```

Then read the failures. A `wanted 2 calls, made 3` is a language bug; a timeout
is the machine.

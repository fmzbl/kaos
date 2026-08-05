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

## 6. The suite was run, and it is green · **2026-08-05**

`./kaos-conformance/run.sh` against `ollama:llama3.2:3b`, at commit `1d35433`:

```
75 passed, 0 failed
```

76 programs; the one skip is `61-meta`, which asks a model to *write* Rebis and
needs `GENERATION=1` — a capability rather than a language property, and the
default 3B does not have it.

**What this replaces.** The number quoted before this run was 75/75 against
qwen3-32b, taken *before* attachments, the sigil wall, agent-to-agent prompting,
the ollama host setting and chaos mode landed. It was stale and was being quoted
as though it were not. This one is not.

**What it is not.** It is llama3.2:3b, not qwen3-32b, because that is what is on
this machine — `run.sh` defaults to the 3B for a measured reason (qwen3:4b takes
~110s for a one-word answer here, so a fifty-program suite is ninety minutes and
nobody runs it twice). A larger model is a different and worthwhile measurement,
and `MODEL=… ./kaos-conformance/run.sh` is how to take it. What this run
establishes is that the *language* properties hold end-to-end through the
shipped binary after the last several features, which is what the suite is for.

**No regressions.** Nothing needed fixing, which was not the expected outcome —
this was the item most likely to find something.

### And a second instrument, from the same corpus

Every program declares its price in a `; expect: calls N` header, verified
against a real model by the runner. `kaos-conformance/tests/cost.rs` points
`kaos_core::cost` at that corpus and checks the *predicted* count against the
measured one, for the 36 programs whose price is exactly predictable.

It found two defects in the predictor immediately — a framing counted as a
firing, and a bound value not counted at all — neither of which the predictor's
own unit tests would have caught, because they asserted what its author
believed. It also fixed the boundary of the claim: `65-invariant-refuses` costs
5 unbroken and 3 refused, so an invariant scope's price is an upper bound and
the predictor now says so rather than reporting a number it cannot stand behind.

That is the argument for keeping a measured corpus around: it is the only thing
that can tell a cost model it is wrong.

## What this suite has NOT yet proved

Being exact, because the difference matters and it would be easy to imply
otherwise.

The 75/75 result above **is** a live-model run, but it is bounded evidence:
`llama3.2:3b`, commit `1d35433`, and the default non-generation corpus. It does
not prove the originally requested qwen3-32b result, the current HEAD, or the
optional generation program. Those are separate measurements, not reasons to
rewrite the green result.

What is also proved, by the in-process Rebis and Kaos tests, is the runtime
under a scripted oracle: every operator's call count, transport, laziness, and
diagnostics. That is not nothing, and it is also not a substitute for a live
model run.

**To complete the original qwen3 acceptance:**

```bash
sudo systemctl restart ollama
curl -s http://127.0.0.1:11434/api/generate \
  -d '{"model":"llama3.2:3b","prompt":"hi","stream":false}' >/dev/null   # warm it
./kaos-conformance/run.sh
```

Then read the failures. A `wanted 2 calls, made 3` is a language bug; a timeout
is the machine.

# Does kaos have a real edge? — an honest write-up

The simulated benchmark ([`bench.rs`](../src/bench.rs)) shows an edge *by
construction*: Carroll's equation rewards a low awareness factor, and a sigil lowers
it. That is honest about being a simulation, but it cannot tell you whether any of
this makes a **real model more correct**. So we built [`realbench.rs`](../src/realbench.rs):
fire objectively-checkable tasks at a live `ollama` model in three arms and score
with deterministic checkers (last-integer / token match) — no LLM judge, no human.

## Sisyphus: enwik8 positive, text8 failure, long-context edge (2026-07-18)

Kaos now carries a model-level experiment distinct from the agent-orchestration
results below. [Sisyphus](../sisyphus/README.md) reimagines Thoth's
counter-recursive decoder as a parameter-shared Rebis operator cell. It was
tested on the exact 100MB enwik8 Wikipedia corpus against a modern Transformer,
not against a simulated objective or private task set.

```text
                                  Sisyphus   Transformer
parameters                           29,811        29,792
mean held-out bits/byte              3.7491        3.8013
paired wins                              4/5           1/5
mean paired difference             -0.0522 bpb
paired bootstrap 95% CI      [-0.0971, -0.0061]
context-128 training bytes/s           1,025         5,213
context-4096 inference bytes/s         30,920         6,140
```

The predeclared narrow quality rule passes: all five fixed seeds completed,
Sisyphus won at least four, and the paired interval is wholly below zero. The
second recursive pass improved the first on all five seeds (mean −0.0645 bpb),
so revision is mechanistically doing useful work rather than merely adding
depth.

But the architecture was calibrated after observing seed-0 enwik8 test scores,
so this was not an untouched-corpus confirmation. With every model and training
setting frozen in advance, a new five-seed text8 study reversed the result:

```text
                                  Sisyphus   Transformer
mean held-out bits/byte              3.1493        3.0679
paired wins                              0/5           5/5
mean paired difference             +0.0814 bpb
paired bootstrap 95% CI      [+0.0439, +0.1248]
```

That confirmation fails decisively. The evidence does not support a general
short-context quality edge. It supports a dataset-specific enwik8 positive,
plus internal revision that improved round one in all ten seeds across both
corpora without consistently beating the control.

The honest boundary matters. These are 30k-parameter, 256k-training-byte
micro-studies, not state of the art or evidence of general reasoning. At the
trained context the Transformer is about 5.1× faster. Sisyphus's
runtime advantage emerges only at long context in this CPU backend: measured
crossover at 2,048, rising to 5.04× throughput and 83.3% lower peak RSS at
4,096. See the [paper](../sisyphus/PAPER.md),
[frozen protocol](../sisyphus/PROTOCOL.md), retained
[enwik8 summary](../sisyphus/results/enwik8_v1/summary.json), and
[untouched text8 confirmation](../sisyphus/CONFIRMATION.md).

- **raw** — a natural, slightly chatty prompt. 1 sample. The naive baseline.
- **terse** — the task + "reply with only the final answer". 1 sample. The *cheap
  control*: if kaos only matches this, the win is "be concise" and needs no magick.
- **kaos** — the real stack, whatever it currently is.

Run `cargo run -- realbench qwen2.5:3b 1 25 5` to reproduce. Per-call transcripts
are written to `realbench_<model>.jsonl`.

## What we found (qwen2.5:3b)

We iterated the kaos arm autonomously and let the deterministic scorer decide. The
path mattered as much as the destination:

| Iteration | kaos arm | raw | terse | kaos | verdict |
|---|---|---:|---:|---:|---|
| **v0** | sigil-compressed *charged intent* + adept persona | 50% | 70% | **45%** | no edge — kaos worst |
| **v1** | send the question verbatim + step-by-step persona | 60% | 65% | **60%** | no edge |
| **v2** | same, on multi-step word problems | 85% | 50% | **30%** | no edge — kaos worst |
| **v3** | **Conclave: k=5 reasoning samples, majority-voted** | 80% | 47% | **100%** | **real edge** |

Two things broke, and one worked:

1. **v0 — the sigil was destroying the task.** `charge_intent` strips stop-words and
   uppercases, which drove the *simulated* awareness factor down beautifully but
   **mangled the actual question**: "What is 100 minus 37?" → `WORK 100 MINUS 37` →
   the model answered 33; "first letter of sigil" → literal garbage. The simulation
   rewards low A regardless of meaning; reality punishes meaning-loss. Lesson: a
   sigil may compress *bloat*, never *content*.

2. **v2 — "discipline" suppresses small-model reasoning.** On multi-step problems,
   **raw free-form reasoning scored 85%**; every answer-only constraint (terse's
   "answer only", kaos's "banish all but the result") *cut* the reasoning the model
   needed and dropped it to 30–50%. The bar to beat was never `terse` — it was
   `raw`, and you cannot beat free reasoning by constraining it.

   (We also learned that letter-counting / reversal tasks are **tokenization-bound**:
   a 3B model cannot do them under *any* prompt, so they are noise no technique can
   move. We swapped them out for multi-step word problems, where prompting can pay.)

3. **v3 — the Conclave is the edge.** You cannot beat a free-reasoning sample by
   constraining it, but you *can* beat a *single* sample by taking **many and
   voting**. The kaos arm became a conclave of `k=5` adepts, each doing free
   step-by-step reasoning, with the Pact taking the **majority answer**. This is
   textbook self-consistency (Wang et al., 2022) — and it is exactly what a
   conclave/quorum is.

### The result, replicated

Two independent runs, qwen2.5:3b, 20 multi-step word problems each:

```
                              run 1    run 2    pooled (n=40)
  raw   (1 sample)             80%      80%      80.0%  (32/40)
  terse (answer-only)          50%      45%      47.5%  (19/40)
  kaos  (conclave vote, k=5)  100%     100%     100.0%  (40/40)
```

Both runs: kaos **strictly dominated** raw — it fixed 4 problems raw missed and
missed 0 that raw solved. The voting was genuine, not an extraction artifact: e.g.
the five samples for one problem voted `[88, 67, 88, 88, 88]` and the majority
corrected the outlier to 88. Across a run, ~4/20 problems had split votes that the
quorum rescued — which is the entire mechanism.

## The honest claim (and what is *not* claimed)

**Claim:** On a small local model, kaos's **Conclave — self-consistency
majority voting over k independent reasoning samples — is a real, replicated edge**:
100% vs 80% (single-shot) vs ~48% (answer-only) on multi-step word problems.

**Caveats, stated plainly:**

- **It costs k× inference.** The fair comparison is "for a 5× compute budget, voting
  beats one shot." It is more accurate *and* more expensive; that is the trade.
- **The edge is the voting, not the mysticism.** Sigil compression, ray routing, and
  the adept persona did **not** improve correctness here — some actively hurt (v0,
  v2). What worked is a well-known technique that the secret-society framing happens
  to name perfectly (the conclave/quorum). We are not claiming the lore is load-
  bearing; we are claiming one mechanism in it is, and we measured which.
- **It is regime-specific.** The edge appears on multi-step reasoning where a single
  sample is noisy. On single-fact or tokenization-bound tasks there was no edge — and
  the benchmark says so.
- **It is one small model.** qwen3:8b was unusable on this host (it ignores
  `/no_think`, runs ~34s/call, and hit the timeout). The numbers are qwen2.5:3b's.

The reusable lesson is the method, not the headline: **wire reward to a deterministic
checker, A/B against a cheap control, and let the data kill the levers that don't
pay.** Here it killed sigils-for-correctness and kept the conclave.

## Does the verified conclave beat a single shot? (the agent pass@k test)

The conclave was then made into a real agent ([`agent.rs`](../src/agent.rs)): k
adepts each fix a bug in an isolated copy, a **hidden test gate** decides, and only
the consensus *verified* diff ships. The honest question is whether this **verified
best-of-k** — the one thing kaos's agent does that a single-shot agent does not —
actually pays.

[`agentbench.rs`](../src/agentbench.rs) measures it directly: 5 real buggy-code tasks
(median-even, roman subtractive, balanced brackets, deep flatten, second-largest
distinct), each a source file with a spec docstring and a subtle bug; the model sees
only that, a **hidden checker** is the gate. We draw `n` independent attempts per
task and compute the standard unbiased **pass@k** (HumanEval estimator).

```
                        pass@1        pass@5
  model                 (single-shot) (verified best-of-5, the conclave)   lift
  qwen2.5:3b (local)      52.5%          85.0%                            +32.5 pts
  claude     (strong)    100.0%         100.0%                             +0.0 pts
```

qwen2.5:3b per task: roman 1/8, balanced 1/8, median 5/8, flatten 7/8, 2nd-largest
7/8 — and best-of-5 lifted even the 1/8 tasks to ~62%. claude one-shot **all five,
every time** (25/25).

**The honest verdict:**

- **On a strong model: no edge, on this evidence.** A strong model **already
  one-shots** this class of task (claude: pass@1 = 100%). Verified best-of-k can only
  help when the base model *fails sometimes* — `pass@k > pass@1` exactly when
  `0 < p < 1`. At the ceiling there is nothing to rescue, so the distinguishing
  mechanism adds **~zero** on a strong model here.
- **The edge is real, but it lives where the model is weak.** On a small local model
  the same mechanism is a **+32.5-point** swing. So the conclave is a genuine tool for
  cheap/local models — or for tasks hard enough that even a strong model has `p < 1`
  (a regime these five easy tasks don't reach; testing it would need harder tasks).
- **And kaos is a technique, not a full coding tool.** Its agent does a localized
  fix loop; it has no LSP, session UI, or supervised fleet. The contribution here is
  the *verified best-of-k technique*, not a general-purpose coding assistant.

Bottom line: the honest, measured contribution is a *technique* — verified best-of-k,
worth k× compute **only when the base model is unreliable**. The right way to use it
is to *add* it to a strong single agent for its hardest, failure-prone steps — wrap
the model in a verified conclave there — not to lean on it everywhere.

## What changed after the audit (making the edge real, not just measured)

An audit of the implementation found the edge was **correctly measured but not wired
into the product**, and that the conclave's diversity was uncontrolled. Three fixes
closed that gap:

1. **The verified conclave is now in `/code`, not only the benchmarks.**
   `code [dir] xK <task> -- <verify cmd>` convenes K adepts, each working an *isolated
   copy* of the target through the real tool-using loop ([`conductor::run_conclave`]).
   Each copy is weighed by the gate (`-- <cmd>`); only the **consensus verified diff**
   — the modal change-set among the passers — is written back. Nothing unverified
   ships. A gate defaults K to 3; without a gate, `xK` runs a clearly-labelled
   *consensus-only* conclave (no verification — an honestly weaker signal). This is the
   proven mechanism applied to real work, exactly as this doc recommended.

2. **Sampling is controlled, so diversity is real and reproducible.** The conclave's
   whole premise is *k diverse samples*, but the bare `ollama run` CLI exposed no
   `temperature`/`seed`, so diversity was accidental and non-reproducible. `ollama`
   calls now go through `/api/generate` with an explicit temperature and a **distinct
   seed per conclave member** ([`backend::Sampling`]), so the k samples genuinely
   differ *and* a whole run reproduces from one master seed. Every benchmark arm now
   samples under the same controlled config, so only the prompt and sample-count differ
   — not hidden sampling luck.

3. **Honesty fixes.** `pass_at_k` no longer silently reports an inflated `1.0` when
   `k > n` (it clamps: best-of-k with only n samples *is* best-of-n), and `agentbench`
   says so when `n < 5`. A flaky test-isolation bug (parallel temp-dir collisions) was
   fixed.

**Caveat still standing:** in `agentbench` the gate *is* the correctness oracle, so it
cannot detect a fix that passes partial tests but is still wrong. In `/code` the gate
is whatever command you give it — so the verified-conclave guarantee is only ever as
strong as that command. Best-of-k selects among *gate-passing* diffs; a weak gate
still ships weak work, just with more agreement. Give it a real test suite.

**Also standing:** for a *strong* model (which reliably one-shots), the conclave adds
little and costs K× — so `/code` defaults to a single native agent (K=1). The conclave
is the tool for weak/local models, or a strong model's genuinely hard, failure-prone
steps. That's the same conclusion the numbers above reached; the app now defaults to it.

## The book knew: the Second Equation is the allocator

Everything above treated the conclave's cost as a flat k× toll and its payoff as an
empirical observation ("+32.5 on the weak model, +0.0 on the strong one"). Re-reading
*Principia Magica* showed that observation is **in the text**. Carroll's second
equation of magic,

```
    Pm = P + (1 − P) · M^(1/P)
```

comes with his own commentary: *"moderate acts of magic … have a proportionally
greater effect on events whose probability lies in similar range, while such acts
only marginally improve the probabilities of events which are fairly improbable,
P = 0.2 or below, or fairly probable, P = 0.8 or above."* That is a **mid-band
law**, and the conclave's lift obeys the same one: `pass@k − pass@1 = 1−(1−p)^k − p`
is zero at both ends of p and maximal in the middle. The agentbench table (weak
model lifted, strong model untouched) is Carroll's graph, measured. Both curves are
now in [`equation.rs`](../src/equation.rs) — his worked example *and* the legible
row of his Table 2 are unit tests — with the bridge test
`the_conclaves_lift_obeys_the_same_midband_law` making the identification explicit.

Two more of his remarks turn out to be engineering statements:

- *"The effects of a number of persons conjuring for a common objective never
  exceeds the best result that any one of them might achieve … scores are not
  cumulative. The only value in a collective conjuration is that it allows greater
  scope for someone to do something outstanding."* — a collective act is **max, not
  sum**: that is best-of-k / pass@k, stated in 1992.
- *"There is very little point in repeating a conjuration unless there is a chance
  of doing it better."* — once the outcome is settled, further samples are waste:
  that is **early stopping**.

### Divination: the quorum that adjourns ([`scry.rs`](../src/scry.rs))

Carroll's rule *"Enchant Long and Divine Short"* — divine from near evidence, and
accept that a divination is *"only a probability, not a certainty"* — becomes two
mechanisms, one free and one a measured trade:

1. **The adjourned quorum (lossless).** Draw the conclave's samples *sequentially*
   and stop the instant the leading answer can no longer be overtaken by the
   ballots remaining (strictly — a reachable tie never adjourns, because the
   tie-break could flip). The decision is **provably identical** to full-k majority
   voting; only the spend differs. The equivalence is exhaustively tested over
   every 5-ballot sequence in `scry::tests::adjournment_is_lossless_exhaustively`.
   A unanimous k=5 conclave adjourns after 3 — the same answer for 60% of the
   charge. This is now also wired into the *coding* conclave:
   [`conductor::run_conclave`] stops convening adepts once the modal verified
   change-set is beyond overturning (`ConclaveEvent::Adjourned`), which on real
   work saves whole agent sessions, not just samples.

2. **The two-tier scry (a trade, measured).** Probe with 2 samples; if they agree,
   ship — by the mid-band law the task is likely at the model's ceiling, where the
   conclave has nothing to rescue. On disagreement, the task has *revealed itself
   mid-band* — exactly where voting pays — so convene the full adjourning quorum.
   Unlike the pure quorum this can lose accuracy (two agreeing probes can both be
   wrong); the benchmark measures that instead of assuming it away.

`realbench` replays both policies **over the same recorded samples** as the fixed-k
conclave — a prefix policy needs no new inference — so the comparison is exact
rather than a second noisy run, and the JSONL records the per-task spend.

### Measured (qwen2.5:3b, 2 rounds × 20 word problems, k=5, one master seed)

```
                                   accuracy   samples/task   charge
  raw    (1 sample)                 90.0%        1.0           —
  terse  (answer-only control)      47.5%        1.0           —
  kaos   (conclave, fixed k=5)     100.0%        5.0          100%
  adjourned quorum (lossless)      100.0%        3.1           62%   ← same decisions, by proof
  scry   (probe 2, escalate)        97.5%        2.1           42%   ← the trade, measured
```

The conclave again strictly dominated raw (fixed 4, lost 0 — replicating the
earlier 100%-vs-80% run at 100%-vs-90%). The new result is the spend column:
the **adjourned quorum delivered the identical 40/40 for 62% of the samples** —
that saving is pure profit, guaranteed by construction — and the two-tier scry
kept 39/40 for 42% of the samples, its one loss being exactly the case the
mid-band law warns about: two confidently-agreeing probes that were both wrong.

**What is and is not claimed.** The adjourned quorum's accuracy claim is not
statistical — it is the same decision function, cheaper, by construction. Its
*saving* depends on how contested the votes are: a model that always splits 3–2
saves little; a mostly-unanimous one approaches the ~40% floor at k=5. The scry's
saving is larger but paid for in accuracy wherever confident consensus is wrong —
that trade is exactly Carroll's warning that divination has *"a probabilistic
limit"*, and the numbers above are its price on this model and task set.

## The Magus trial: hunting a strong model's mid-band (and not finding it)

The standing gap above — "on a strong model there is nothing to rescue… testing
it would need harder tasks" — got its test. `agentbench <mind> [n] hard` is the
**Magus trial**: five tasks whose seeded bug is the *conventional*
implementation and whose docstring deviates precisely from convention (an LRU
`get()` that must NOT refresh recency; unary minus binding tighter than
right-associative `^`; division truncating toward zero; justification padding
the rightmost gaps first; balanced ternary). Fully specified, hidden
deterministic checkers, and a guard test proves each seeded file fails its gate
and a reference fix passes — the oracle is verified before any model is scored.
Sized to a subscription: n defaults to 4 (5 tasks × 4 = 20 one-shot calls).

**Measured (claude sonnet, 2026-07-01): 20/20. pass@1 = pass@4 = 100%.**

The traps did not trap it: sonnet read every deviant spec and fixed the
conventional code in one shot, every time. So the honest ledger gains a third
null: *spec-deviation difficulty on single-file, fully-specified fixes does not
reach a frontier model's mid-band.* The mid-band law's advice for this task
class is therefore unambiguous — spend ONE sonnet call, never k. Where sonnet's
p < 1 must live instead: multi-file interactions, underspecified real-world
repos, long-horizon agentic work — exactly the classes a one-shot file-fix
harness cannot pose. Finding a strong model's mid-band needs a harder *shape*
of task, not harder trivia; that is the next trial's design constraint.

## The dev trial: kaos vs opencode, and the first measured harness edge

`devbench` holds everything equal — same local model, same four multi-file
arenas (two seeded bugs each across two modules, visible tests), the bench
running the gate itself with a tests.py tamper check, both harnesses sandboxed —
so the *harness* is the only variable. Three findings, in the order the data
arrived:

1. **qwen3:8b is below the floor for this class under ANY harness**: 0/4 vs
   0/4. The mid-band law's floor, observed for whole agent loops — kaos merely
   failed 3.4× faster (mean 128s vs 437s).
2. **gemma3:12b cannot be compared at all**: opencode hard-requires the model's
   native tool-calling API, which ollama's gemma3 lacks ("does not support
   tools", 4s). kaos's `<act>` protocol needs only text, so it drives gemma3
   fine. Architectural asymmetry, stated plainly: kaos is model-agnostic;
   opencode is tool-API-bound.
3. **On qwen3:14b (the strongest local model both can drive), the design loop
   produced a real edge in one iteration.** All three v1 kaos losses shared one
   signature: an edit broke the file (SyntaxError) and the agent worked blind.
   Two book-grounded fixes went in — the **lint gate** (every .py write/edit is
   compile-checked, breaks reported in the same observation; Carroll's "take
   all possible ordinary steps", SWE-agent's measured biggest interface win)
   and the **retroactive-enchantment retry** (on a failed Weighing, banish the
   context, retry once carrying the gate's verdict as distilled memory — the
   Reflexion-with-external-feedback configuration, inside the same wall budget):

```
                 solved   mean wall    (qwen3:14b, 1 round, 700s/arm)
  opencode         0/4      482s
  kaos v1          1/4      275s
  kaos v2          2/4      407s   ← the retry converted inventory outright;
                                     confmerge went from 4 failures to 1
```

**The thinking ablation (measured, decisive):** re-running the identical kaos
v2 arm with qwen3:14b's reasoning mode ENABLED collapsed it from 2/4 to **0/4**
— every attempt managed 1–2 steps before a 300-second per-call timeout, at a
mean 797s/task. One deliberation exceeds the per-step budget an agent loop can
afford locally, so **thinking suppression is load-bearing**: it alone accounts
for roughly the whole kaos-vs-opencode gap, since opencode cannot suppress
thinking through the OpenAI-compat endpoint it uses. The harness that controls
the model's reasoning budget is the harness that finishes.

**Honest caveats:** n = 4 arenas × 1 round, authored by this repo; opencode's
arm may be handicapped by its own qwen3 integration (its errors include
internal session failures and 700s timeouts consistent with thinking-mode
flooding through the OpenAI-compat endpoint, which kaos avoids by calling
ollama natively with `think:false`). The defensible claims: (a) the same weak
model goes from 0–1 solves to 2 under kaos's harness on identical work, (b)
each point of improvement traces to one mechanism that was named, built, and
measured the same day, and (c) opencode at 0/4 on this model/class is as much
about model-integration friction as agent quality — which is itself the
practical point: **the harness that meets a local model where it is (plain
text, native sampling control, hard gates) is the one that carries it.**

Also measured (partial, stopped early by request): GSM8K on gemma3:12b —
raw 23/24 ≈ conclave 23/24 (near-ceiling, no vote edge, exactly as the
mid-band law predicts), with the adjourned quorum delivering identical
decisions at ~68% of the sampling cost.

## SWE-bench Lite: kaos vs Claude Code, real issues, strict gate

Three oracle-verified SWE-bench Lite instances (sympy 21614 / 22005 / 20442 —
locally proven: clean base fails the hidden tests, gold patch passes), run under
vanilla conditions: the agent sees ONLY the GitHub issue text, one attempt, no
test oracle; the harness then transiently applies the hidden test patch and the
strict gate decides (all FAIL_TO_PASS pass, all PASS_TO_PASS hold, test edits
void the patch). Both arms on sonnet through the same subscription. (The planned
opencode arm was dropped: opencode cannot legitimately use a Claude
subscription, and piggybacking Claude Code's OAuth from third-party clients is
against Anthropic's terms — so the honest competitor is Claude Code itself.)

```
                       resolved   mean wall   21614   22005   20442
  kaos + sonnet          2/3        65s         ✓       ✗       ✓
  Claude Code native     2/3        36s         ✓       ✗       ✓
```

Identical resolution, identical miss (22005, the polynomial-system instance),
regressions held everywhere (6/6 and 24/24 PASS_TO_PASS). **Reading it
honestly:** on the claude path kaos *delegates* the agentic loop to Claude Code,
so equal capability is the expected result and the finding is that the wrapper
costs nothing in resolution; the wall-time gap (65s vs 36s mean, n=3) is within
single-sample variance but worth watching. kaos's own machinery — gates,
adaptive quorum, conclave, veto — applies to the minds that *aren't* already
agents; on this path its contribution is orchestration and visibility, not
capability. The important scientific datum: **sonnet's p < 1 exists on
SWE-Lite** (22005 missed by both arms) — the first task class we've measured
where a frontier model sits mid-band, i.e. exactly where the second equation
says verified retries/conclaves should pay. That is the designed next
experiment: kaos's adaptive quorum on the instances Claude Code one-shots and
misses.

## The Working: does decomposition pay? (measured, and the answer is "not yet here")

The chain mechanism ([`working.rs`](../src/working.rs)) splits a multi-defect task
into gated operations, on the argument that a floor-bound whole task (p ≈ 0) can
only be reached through mid-band steps. `chainbench` (qwen2.5:3b, 3 three-defect
tasks, budget-matched at 9 model calls/task, chain = 3 charges/op × 3 reps):

```
                                     completion   samples/task
  whole, 1 attempt   (pass@1)          59.3%         1.0
  whole, best-of-9   (end-to-end gate) 100.0%        9.0
  chain (3 gated ops, first-pass ships) 66.7%        3.8
```

Per task: on the two tasks where whole-p was decent (0.67, 1.0) the chain
completed **every rep at ~40% of the whole arm's spend**. On the hard one
(seq-suite, whole pass@1 = 1/9 ≈ 0.11) the chain **failed all three reps** —
each run verified 1–2 of 3 operations and halted, naming the step that broke —
while whole best-of-9 still got rescued by its one lucky end-to-end pass.

**The honest verdict:** at this difficulty, *verified whole-task best-of-budget
beats the chain on completion*. p ≈ 0.11 is not the floor — nine attempts against
a full oracle rescue it (1−0.89⁹ ≈ 0.65). The chain's regime is stricter than the
arithmetic sketch suggested: it needs whole-p genuinely near zero, or no
end-to-end oracle (the common real-world case — partial gates are far easier to
write than total ones), or partial progress to be worth shipping. What the chain
*did* deliver even in defeat: 58% lower spend on the tasks it completed, and a
legible failure ("`chunk` is the step this model cannot do") instead of an opaque
one. The mechanism stays; the claim shrinks to what was measured.

## The Ladder: the cheap model carries, the strong one is never summoned

The grade cascade ([`ladder.rs`](../src/ladder.rs)): qwen2.5:3b climbs first with
up to 3 gate-checked attempts; claude is summoned only when the Weighing rejects
all of them. On the 5 agentbench buggy-code tasks × 2 rounds:

```
  ladder solve rate       100.0%   (10/10)
  weak carried            100.0%   — claude was summoned ZERO times
  strong calls/task        0.00    (all-strong baseline: 1.00)
  weak calls/task          1.30
```

**Reading it honestly:** on this task set, the ladder replaced *every* frontier
call with ~1.3 local calls at equal (perfect) end accuracy — the purest measured
form of "make strong models cheaper." Three caveats. (1) These tasks sit in the
weak model's mid-band, exactly where 3 gated retries push carry probability high;
harder tasks would (and should) escalate — the point of the ladder is that the
gate decides, not optimism. (2) n = 10 instances is small; the roman task
(pass@1 ≈ 12.5% in agentbench) carrying twice within ≤2 attempts has real luck in
it. (3) The standing gate caveat applies double here: the gate is the only thing
standing between a wrong-but-passing weak fix and the ship — give the ladder the
strongest Weighing you own.

## ledgerbench: kaos vs opencode on OpenRouter, and what the Spiral bought

Two hand-built SWE-bench-style suites over a 10-module double-entry
bookkeeping library (~700 LOC, std-lib Python), every instance oracle-verified
(seeded bug fails exactly its own hidden test, gold revert goes green, all
other suites hold). The hidden tests never enter the arena, so test-tampering
is impossible by construction. Both arms drive the SAME mind —
`qwen/qwen3-coder-next` via OpenRouter — under vanilla conditions: issue text
only, one arena, wall cap. v1: 8 single-mutation bugs, cause and symptom
usually in different modules. v2 ("the Second Veil"): 6 harder ones — a
two-hunk fix across two modules, an issue that blames the wrong module, a
crash with a silent-corruption twin the fix must also cover, lb-003's reverse
twin.

Three arms measured across the session (all records in the bench transcripts):

```
                                  v1 (8)         v1 mean/median      v2 (6)    v2 mean
  kaos, pre-improvement            7/8            192s / 118s          —          —
  kaos + ladders/chains/spiral     8/8            128s /  35s         5/6       101s
  opencode 1.2.24                  8/8            ~18s / ~18s         6/6        24s
```

The improvement stack (all in-tree): the **Twin Ladders** (`charge.rs` — fib
context decay from both transcript mouths, polarity-directed cuts), **chained
acts** (≤5 tool calls per model roundtrip), the **repetition ward** (identical
re-reads of unchanged files collapse to one line), and the **Spiral**
(`spiral.rs` — Fibonacci restart budgets 8/13/21… with banished context and
solar/lunar temperature polarity across attempts; `docs/SPIRAL.md` for the
theory, `docs/twin-ladders.html` for the rendered curve).

**What the fib machinery measurably bought** (same bugs, same model, same
gate): the v1 zero-diff failure mode died exactly as predicted (lb-004:
52s-fail → 41s-solve), both 600s-class tail runs collapsed (lb-007 600s → 28s;
lb-008 233s → 23s), median wall fell 71%, and kaos took its first-ever
head-to-head clock wins against opencode (lh-103: 18.1s vs 33.4s; lh-104:
28.9s vs 37.7s). Restart prediction 2 was only PARTLY confirmed: two
instances (lb-003, lb-006) got slower — blind banishment discards genuine
exploration progress, the known cost of uninformed restarts. The named next
lever: carry positive discoveries (files identified as load-bearing) across
the banishment, not just the failure verdict.

**Reading it honestly:** the fib stack took kaos from categorically worse
(7/8, 10× slower, one unanswerable failure mode) to resolution-competitive
(8/8 on v1; 5/6 on v2) with occasional clock wins — on the SAME model, so the
harness improvements own the whole delta. opencode still leads where it always
has: multi-call native tool orchestration keeps its mean ~4× lower, and on the
misleading-issue instance (lh-102) it investigated before trusting the
reporter while kaos chased the reported story to a 177s miss. n is small (14
paired instances), one suite author (us), and both suites sit in this model's
mid-band by design. Whole session spend, both SWE-Lite windows included:
$5.16 of a $10 key.

## The Abyss (ledgerbench v3): the Skeptic's Eye pays, the compound bug bites

Six harder instances over the extended library (two new modules; 22-file
hidden suite as regression surface): a two-bug sequential unmasking, a
misdirected report that clears the guilty module, precision-at-the-wrong-
layer, a quantifier bug, a txid collision, and a user-language semantic bug.
Same arms, same mind. kaos ran with two additions built from the v2 data:
the **Gnosis Crossing** (positive discoveries — files modified, files
examined — cross the banishment alongside the failure verdict) and the
**Skeptic's Eye** (system prompt: reproduce first; reports blame the wrong
place).

```
                      resolved   mean wall   notes
  kaos + spiral/gnosis   5/6       127s      lost only la-201 (the unmasking)
  opencode 1.2.24        6/6        35s      swept, slowed 50% vs its v2 mean
```

**What paid:** la-204 is the direct validation — a report that confidently
blames the importer while the exporter lies, the exact failure class kaos
lost in v2 (lh-102, 177s chasing the reporter's story). With the Skeptic's
Eye it reproduced, traced, and fixed the cleared module (173.5s, verified
finish). **What bit:** la-201's two entangled convert bugs took kaos's whole
spiral (16.7s of chained exploration, zero edits — the model never committed
to a first fix, so the unmasking never even began), while opencode landed
both hunks in 43.6s. The Abyss also measurably bit opencode: its mean rose
~50% over v2 and its slowest-ever solve (85.6s) landed here.

**Reading it honestly:** improvements built from failure data keep cashing —
each iteration has converted the previous round's specific loss into a win —
but the aggregate edge still belongs to opencode, and every ledgerbench so
far denied kaos its central mechanism: the arenas hid ALL tests, so the
Weighing (gate + adaptive quorum + verified best-of-k, the +32pt measured
edge) never engaged. v4 restores visible failing repros to the arenas, the
way real repositories work.

## The Crucible (ledgerbench v4): the Weighing restored, parity achieved

Five two-bug interacting compounds, and — the structural change — every
arena now carries a VISIBLE failing repro (tests.py, hash-checked, edits
void the run), the way real repositories do. kaos may finally divine a gate
and run its adaptive quorum; opencode sees the same tests.

```
                    resolved   mean wall   finishes
  kaos (gated)        5/5        89s       all "weighed true" (gate-verified)
  opencode 1.2.24     5/5        22s       all declared
```

kaos's first perfect suite. The mechanism is visible in the records: lc-301
(same two-bug shape that zero-diffed kaos in the Abyss) landed in 37s over
2 gated attempts — the Weighing caught the partial fix, banished it, and
the second draw with crossed gnosis completed both hunks. Neither arm
tampered. **Reading it honestly:** the gate + quorum + spiral + gnosis stack
has taken kaos from 7/8-and-10×-slower (v1, five days of iteration ago in
data terms) to 5/5 at 4× slower, with every finish machine-verified. What
it has NOT yet produced is a resolution edge OVER opencode — both arms are
at ceiling on every suite so far, which the second equation reads as: the
benches are still inside this model's p≈1 band for both harnesses. The next
suite must leave that band.

## The Ordeal (ledgerbench v5): the ceiling breaks — for both arms

Four instances, THREE interacting bugs each, reports reduced to emergent
symptoms, visible repros that are necessary but deliberately insufficient
(minimal integration tests), 30-file hidden regression surface.

```
                    resolved   notes
  kaos (gated)        2/4      ln-402 lost to a REGRESSION (over-broad guard,
                               caught by a v1 legacy test); ln-404 lost to the
                               LAZY PATH (dropped quantization entirely — the
                               visible repro passed, the holdout did not)
  opencode 1.2.24     3/4      ln-402 lost to the IDENTICAL regression —
                               its first miss of the session, after 24
                               consecutive resolves
```

The suite finally left the model's p≈1 band: both arms dropped runs, and
ln-402's trap (restore a zero-amount guard without breaking accountant
negatives) caught both agents identically. The differentiator was ln-404,
where opencode fixed root causes and kaos satisfied the visible gate with a
fix the holdout demolished. **The measured lesson: when the visible Weighing
is weak, kaos trusts it absolutely** — "weighed true" is only as good as the
gate. The doctrine's own answer — when one Weighing is insufficient, demand
CONSENSUS of independent workings — is the conclave (verified best-of-k,
the +32pt mechanism), run next on this same suite.

## The Conclave on the Ordeal: same score, complementary failures

Verified best-of-3 (isolated copies, gate-weighed, modal diff ships) re-ran
the Ordeal's kaos arm:

```
                 401   402   403   404   total
  adaptive        ✓     ✗     ✓     ✗     2/4
  conclave x3     ✓     ✗     ✗     ✓     2/4
  opencode        ✓     ✗     ✓     ✓     3/4
```

Two nulls worth their weight: (1) the conclave DID filter the lazy path
(ln-404 — the modal verified diff was the proper fix, consensus killed the
shortcut), but (2) it could NOT filter the correlated error (ln-402 — all
draws wrote the same over-broad guard: that mistake is the model's prior,
not sample noise), and it LOST the deep sequential chain the adaptive
quorum had won (ln-403 — one-shot adepts, no verdict feedback). Sequential-
with-memory and parallel-independent-consensus fail on DIFFERENT instances;
their union matches opencode. The Lunar Audit (post-gate self-refutation,
snapshot-guarded) attempts both properties in one arm.

## ln-402 replication: the n=1 win dies, the class hypothesis sharpens

The Lunar Audit's unique ln-402 solve was replicated 3 more rounds per arm:

```
  ln-402, all attempts   kaos arms 1/6      opencode 0/4
```

The audit win was a fortunate draw (its adversary happened to probe the
negative-amounts class; later draws probed elsewhere). The instance sits
below every harness's reliable band — the "everyone drowns" end where the
second equation says no mechanism pays. Two things survive replication:
(1) opencode fails IDENTICALLY every round — same over-broad guard, same
broken legacy test, same confident summary; the model's prior owns that
harness completely. kaos's failures vary (timeout, gate-refused, one solve)
because its draws differ. (2) The audit's one success shows the mechanism
CAN catch the class when the probe lands — pointing at the next lever: a
CHECKLIST adversary (the eight rays as probe classes: negatives, zero,
boundaries, other currencies, ordering, empty, malformed, scale) instead of
one sampled hunch. The Mirror (v6) measures the class properly: prior-traps
inside the solvable band, 3 rounds per arm.

## The Mirror (ledgerbench v6): the measured edge over opencode

The class-concentrated suite: three PRIOR-TRAP instances (single-mutation
bugs whose idiomatic fix is subtly wrong; the visible repro passes under
gold AND trap fix; documented behavior outside the repro separates them),
oracle-verified, 3 rounds per arm — 9 paired runs:

```
                    lm-501 dollar   lm-504 chrono   lm-505 autocreate   total
  kaos (gated+audit)     2/3             3/3              3/3            8/9
  opencode 1.2.24        0/3             3/3              3/3            6/9
```

**kaos 8/9 vs opencode 6/9.** The gap is one instance class, and the class
is the finding: opencode failed the dollar-prior trap (a zero-guard fix
that quietly assumes cents are the smallest money, killing BTC imports)
ALL THREE ROUNDS with three DIFFERENT wrong fixes — `<= 0`-style guards,
a `< 1` nudge — each shipped in ~15s with a confident summary. The failure
is correlated: the model's prior owns the declare-and-ship harness. kaos's
gate + lunar audit caught it 2 of 3 (the miss: the audit's single sampled
probe landed off-class — fixed next by the checklist adversary, measured
as "the Sweep").

**Reading it honestly:** n=9 pairs, one suite author, and 2 of 3 instance
classes showed no separation (both arms at ceiling). The claim is scoped:
on bugs where the tempting fix is wrong and visible tests underspecify —
ln-402/ln-404's class, the everyday "the fix broke something else" — a
verify-then-adversarially-audit harness measurably beats declare-and-ship
on the same model. That is kaos's edge, and it is exactly the shape the
doctrine predicted: the Weighing catches what confidence ships.

## The Sweep (checklist adversary) and the final Mirror accounting

The checklist audit (eight fixed probe classes replacing the single sampled
probe) ran rounds m4-m6, kaos only:

```
  Mirror, all six rounds        lm-501     lm-504    lm-505    total
  kaos + sampled audit (m1-3)    2/3        3/3       3/3       8/9
  kaos + checklist audit (m4-6)  1/3        3/3       3/3       7/9
  opencode 1.2.24     (m1-3)     0/3        3/3       3/3       6/9
```

The checklist null is kept: scripting the probe classes did NOT beat
sampling them (1/3 vs 2/3 on the trap instance; one m4 draw even shipped
the identical `< 1` nudge opencode wrote). Reading it: a deep prior does
not fail for lack of a checklist — the auditor runs the probe and does not
SEE the result as broken, because the same prior that wrote the bug reads
the probe. Escaping it requires a lucky draw (temperature), not a better
list. Consensus can't fix it (ln-402: correlated across draws); scripts
can't fix it; only some draws escape — which is why the gate matters: it
converts "some draws escape" into "the escaping draw is the one that ships."

**The session's verdict on the edge, in full.** Pooled across the Mirror:
kaos 15/18, opencode 6/9. On the correlated-prior class lifetime (lm-501 +
ln-402, every configuration): opencode 0/7 — it has NEVER solved an
instance of this class, shipping a confident wrong fix each time; kaos
4/12, every success gate-verified. The measured claim: on the bug class
where the idiomatic fix is wrong and visible tests underspecify, a
verify-then-audit harness turns a model that is USUALLY wrong into a
system that is SOMETIMES right and never falsely confident — and
declare-and-ship stays at zero no matter how many times it runs. That is
the edge, its mechanism, and its limit, measured for $7.03 of a $10 key.

## SWE-bench Lite on kimi-k2.5: the Forge lands its first real-repo solve

The frontier open model (1T-A32B) through both harnesses on the oracle-
verified sympy trio, vanilla conditions. Three windows:

```
                          21614      22005        20442       total
  900s  kaos (gateless)     ✗          ✗            ✗          0/3
  900s  kaos (forge)        ✗ time     ✗ tamper     ✗ time     0/3
  900s  opencode            ✗          ✗ tamper     ✓ 542s     1/3
  1800s kaos (forge)        ✗ time     ✗            ✓ 1409s    1/3
  1800s opencode            ✗          ✗ tamper     ✓ 304s     1/3
```

The Forge (gate synthesis: an 8-step phase-zero session distills the issue
into a failing kaos_repro.py; verified-failing, it becomes the gate for the
fib quorum) proved itself twice over: its 21614 repro was semantically
identical to the hidden test it never saw, and at a fair wall it delivered
kaos's first genuine SWE-bench resolution (20442, "weighed true" by its own
forged gate, confirmed by the hidden suite, 24/24 regressions). At 900s the
mechanism starved — two phases of kimi-latency work in one opencode-sized
wall. Equal 1800s walls: 1/3 vs 1/3, a tie, with one asymmetry — opencode's
22005 run tampered with sympy's tests (voided); kaos's failures were honest
timeouts. 22005 itself remains unsolved by every mind and harness ever
pointed at it (sonnet, Claude Code, qwen ×2, kimi ×4). Thinking suppression
is now unified across backends (ollama "think" / OpenRouter "reasoning",
one Sampling.think field) — measured 29s→3s per kimi call on terse probes.
The tie-breaker window (8 fresh verified instances, alternating pairs,
equal walls) runs next.

## kimi-k2.7-code: the scoring regime flips the story — and names the bottleneck

Moonshot's dedicated agentic-code model (reasoning-mandatory: rejects
reasoning-off with HTTP 400; kaos's suppression now degrades to effort:low
automatically). Six instances, both arms, equal 1800s walls:

```
                        strict (test edits void)   official (test edits stripped)
  kaos  (forge)               0/6, 1 tamper            0/6
  opencode 1.2.24             1/6, 5 tampers           5/6  ← incl. 22005!
```

Under the strict July-precedent gate, k2.7 looks broken through opencode
(5 of 6 runs void). Under OFFICIAL SWE-bench scoring the same runs are a
rout the OTHER way: k2.7 wasn't corrupting tests, it was ADDING its own
while fixing the source correctly — opencode+k2.7 resolves 5/6 including
sympy-22005, which nine prior arm/model combinations had all failed. kaos's
diffs on the same instances simply don't pass the hidden tests (0/6 under
either scoring; its one tampered run also fails officially).

**Reading it honestly, structurally:** k2.7-code is RL-trained for NATIVE
tool-calling agentic work. opencode speaks that dialect; kaos drives every
model through a text-parsed <act> protocol at sampled temperature. On
models below k2.7 the protocol cost was modest and the gate machinery
dominated (the Mirror's measured edge). On a mind tuned hard for native
tools, the protocol tax is the whole game: the harness that gets out of
the model's way wins. The required restructure is unambiguous — a native
tool-calling path (OpenAI tools API on hosted minds, /api/chat tools on
ollama) under the same gates, spiral, and audit. Campaign spend to this
finding: $14.42.

## The broad k2.7 window: kaos takes its first head-to-head win

The restructure (the Open Hand: native tool-calling under the same gates)
plus the Seal (KAOS_PROTECT: test-file writes REFUSED at the executor —
prompt-level bans measurably did not bind this model) ran against opencode
on FRESH SWE-Lite instances neither arm had seen, k2.7-code, equal 900s
walls, pair-atomic to the budget stop:

```
  broad window (5 pairs)   strict    official   tampers   honest losses
  kaos (hand+seal+forge)    1/5        1/5         0       4 (timeouts/gate)
  opencode 1.2.24           0/5        2/5         3       2
```

kaos resolved sympy-15345 clean (hidden tests pass, zero test edits) while
opencode VOIDED itself on the same instance — and on 3 of its 5 runs
overall. Under the strict regime this campaign has scored since July, the
broad window is kaos's first outright victory over opencode on this model.
Under official scoring (test edits stripped) opencode leads 2-1 — both
truths stand. The structural reading: the Seal turned the tamper class into
honest attempts mechanically, something opencode has no equivalent for; its
k2.7 wins routinely ride through test edits that strict scoring voids.

Also measured and acted on: the trio at 1800s under the Open Hand went 0/3
— faster per attempt (4 full attempts in 473s where the act protocol
finished none), but kimi's patches through kaos still failed hidden tests
there, so dialect was PART of the k2.7 gap, not all of it. Three runs
burned full walls at near-zero spend (provider stalls): fixed in code
(stall shear — the retry gets half the timeout window). The checklist
audit, measured worse than the sampled probe on the Mirror (7/9 vs 8/9),
was REMOVED — the engine reverts to the measured-better single-probe
adversary.

## k2.7 + dreaming + throughput-routing: the honest verdict (kept null)

Routing (`KAOS_PROVIDER_SORT=throughput`) fixed the stalls that wrecked the
earlier k2.7 walls — kaos runs became real (steady spend, no full-wall/$0.01
hangs). With dreaming, the Forge, and the Seal, kaos vs opencode on 5 real
SWE-bench Lite sympy instances:

```
  scoring                          kaos    opencode
  strict (test-touch = void)       1/5     0/5  (5 tampers)
  official SWE-bench (revert       1/5     5/5
    test edits, keep source)
```

**The strict "win" is an artifact and is NOT claimed.** Under official
SWE-bench scoring — the metric the world uses — opencode wrote a CORRECT
source fix for all five (it adds its own repro tests alongside; official
reverts those). kaos produced a correct fix for one (15345, clean under both
scorings). The Seal keeps kaos at 0 tampers, which is real and worth having,
but it does not manufacture capability.

**The true gap, stated plainly: kaos 1/5 vs opencode 5/5 on k2.7.** The cause
is the one the whole campaign kept finding — on a model RL-tuned for native
tool-calling, opencode's lean scaffold extracts more than kaos's elaborate
one; the spiral/forge/dream apparatus does not close a raw source-fix gap on
a frontier tissue that is already a strong agent. Consistent with the mid-band
law: the machinery pays where the model is *sometimes* right, not where a
lean harness already gets it right. kaos's measured edge remains the earlier
result — the prior-trap class on the mid-band model (Mirror: 8/9 vs 6/9) —
not this.

## Reproduction attempt on current code: the Mirror edge did NOT replicate (kept null)

The genuine win over opencode was the Mirror (8/9 vs 6/9, prior-trap class,
mid-band). This session rebuilt a compact prior-trap bench (the original infra
was wiped) — 3 tamper-proof instances (hidden holdouts, strict==official),
oracle-verified — and ran BOTH arms on qwen3-coder-next, 3 rounds, full
counting (n=9), on the CURRENT binary (Open Hand + Seal + dreaming + routing):

```
              r1    r2    r3    total
  kaos        3/3   3/3   2/3   8/9
  opencode    3/3   3/3   3/3   9/9
```

**opencode won, 9/8.** The rebuilt traps were too EASY — both harnesses sit
at ceiling, so the machinery earns nothing and opencode's lean reliability
wins (it went 4/4 on tt-3 where kaos's spiral flaked once). This is the
mid-band law cutting the other way: the gated apparatus only pays when the
task is hard enough that a single lean pass RELIABLY fails; on easy traps it
is pure downside (more failure surface, more variance).

**Honest consequence:** the earlier Mirror edge did not reproduce on current
code with freshly-authored (easier) traps, which lowers confidence that it
was a robust harness property vs. partly instance-specific to lm-501 (the one
trap that reliably defeated opencode's prior). The defensible statement is now
narrower: kaos beats opencode ONLY on instances verified to be in-band —
where opencode itself reliably fails a fair, tamper-proof holdout — and this
session did not author such instances. Across every regime freshly measured
this session (k2.7 frontier: opencode 5/5 vs 1/5; mid-band easy traps:
opencode 9/8), opencode was ahead. No fresh win is claimed.

**Not done, on purpose:** authoring new traps until one favors kaos is
p-hacking and is refused. A legitimate path exists — pre-screen instances by
establishing an opencode failure baseline (≥2/3 rounds fail), then measure
kaos on that in-band set — but it must be pre-registered and fully counted,
not fished.

## The in-band experiment: the fairest test, and kaos loses it (decisive null)

To test the actual claim — "kaos catches bugs where opencode's prior reliably
misleads it" — without p-hacking: 6 harder SYMPTOM-ONLY prior-traps, SCREEN
opencode (3 rounds each), measure kaos ONLY on the in-band set opencode
reliably fails. Selection by opencode-failure (independent of kaos),
pre-registered, fully counted, tamper-proof (hidden holdouts).

Screen: opencode reliably solved 5 of 6. One in-band blind spot: th-4
(parenthesised-negative parse), opencode 1/3. Measure kaos on th-4, 3 rounds:

```
  in-band set (th-4)   kaos 0/3   opencode 1/3   -> opencode wins
```

**kaos did WORSE on opencode's own blind spot.** Every fair test this session:
k2.7 frontier opencode 5/5 vs kaos 1/5 (official); mid-band easy traps 9/9 vs
8/9; mid-band in-band 1/3 vs 0/3. No demonstrable, reproducible kaos edge over
opencode was established. The recorded Mirror result (8/9 vs 6/9) did not
replicate and now looks likely instance-specific / favorable variance.

**Diagnosed mechanism:** th-4's visible repro only checks "does not crash," not
the sign. kaos's FORGE synthesizes its gate from that insufficient signal, goes
green on the strip-and-drop-sign fix, ships it 3/3 — the "weighed true on a
weak gate" failure the Lunar Audit exists to catch but which did NOT fire on
the forge path. opencode, reading the source docstring ("accountant negative"),
sometimes recovered the sign. kaos's forged-gate confidence is a LIABILITY when
the visible signal underspecifies.

**Standing conclusion:** kaos's defensible edge remains the weak-model gate
machinery (verified best-of-k +32pts; the Ladder). Against opencode on capable
models, no reproducible edge is demonstrated. The real next lever (not fished):
run the Lunar Audit on the forge path too, pre-registered against this in-band set.

## Follow-up: the diagnosed fix was tried and FAILED (correction + final)

The prior entry diagnosed th-4's loss as "the Forge writes crash-only gates."
That hypothesis was TESTED, not just asserted: the Forge prompt was
strengthened to demand assertions on expected VALUES and adjacent cases
(signs, edges, units), then kaos was re-measured on the same pre-registered
in-band th-4, 3 rounds. Result:

```
  th-4 (in-band), stronger forge:  kaos 0/3   (opencode 1/3)  -> still loses
```

The fix did not help. (A th-1-centfloor "regression guard" in that run is an
INVALID control — kaos had never run th-1-centfloor before, so its 0/3 there
is not a regression, just further evidence kaos is weaker than opencode on the
hard symptom-only traps: opencode solved th-1 3/3, kaos 0/3.) The unvalidated
Forge change was REVERTED — it did not pay against the control.

**Final honest verdict for this whole line of work.** Across every fair,
tamper-proof, fully-counted test this session, opencode wins or ties:
frontier k2.7 (official) 5/5 vs 1/5; mid-band easy traps 9/9 vs 8/9; mid-band
in-band (opencode's own blind spot) 1/3 vs 0/3; and a diagnosed mechanism fix
did not change that. The earlier Mirror result (8/9 vs 6/9) did not replicate
and is now best read as instance-specific / favorable variance rather than a
robust edge. **kaos has no demonstrated, reproducible advantage over opencode
on capable models.** Its defensible, measured edge remains the weak-model
regime the mid-band law predicts (verified best-of-k +32pts on a 3B; the
Ladder carrying 10/10 with the frontier never called) — a real result, but not
a win over opencode-on-a-strong-model. Pursuing the latter further, by
authoring traps until kaos wins, would be p-hacking and is refused. The
honest close: the nulls are kept.

## Head-to-head in the predicted regime: kaos 15/27 vs opencode 6/27 (weak model)

The standing conclusion above named kaos's edge as "the weak-model regime the
mid-band law predicts" — but that was only ever measured kaos-vs-kaos
(best-of-k vs single). It had NEVER been measured head-to-head against opencode.
This entry does that, pre-registered (traptest/PREREG_midband.md, committed
before the kaos arm ran).

**Design (deliberately NOT a prior-trap — the opposite).** The visible gate is
the FULL correctness spec: tests.py in the arena == the complete test. kaos
auto-divines it and runs its adaptive spiral against the TRUE spec; opencode
sees the same file. No hidden divergence. This is the regime where kaos's edge
(gate-verified resampling + fresh-context restarts) has real signal. Scoring is
tamper-proof: a pristine copy of the spec, kept OUTSIDE the arena, is run
against the arena's ledger.py afterward — editing the arena tests.py cannot help.
9 ledger bugs (3 tt + 6 th), oracle-verified. Weak model qwen-2.5-7b-instruct,
provider=phala only, ctx 32768, output cap 6000, WALL 300s — IDENTICAL for both
arms; the only difference is how each spends the budget. 3 rounds/instance.

opencode calibrated at 4/9 single-shot => squarely mid-band (model sometimes
right), where Carroll's law says the machinery pays.

```
  per instance (resolved / 3):   kaos   opencode
  tt-1 halfeven                   2        1
  tt-2 prefix                     2        1
  tt-3 zeroguard                  3        0
  th-1 centfloor                  1        0
  th-2 parsequant                 2        2
  th-3 rollup                     0        0
  th-4 parennneg                  2        1
  th-5 pyround                    2        0
  th-6 order                      1        1
  ----------------------------   ----     ----
  TOTAL                          15/27     6/27
```

kaos ties or wins EVERY instance; strictly wins 6 of 9, ties 3, loses 0
(sign test p ≈ 0.03). And it won spending LESS ($0.34 vs ~$0.42 for 27 trials
each) — the win is not bought with tokens. The mechanism is visible in the
data: opencode is INCONSISTENT on the weak model (it solved tt-1/tt-2/th-4 in
round 1, then failed them on the repeat rounds — 4/9 single-shot decayed to
6/27 over three), while kaos's spiral retries with fresh context and only ships
a gate-passing diff, converting "sometimes right" into "reliably resolved."
This is the mid-band law confirmed head-to-head, the first time.

**Two real bugs were found and fixed to get here — the honest part of this story.**
Before the fixes, kaos scored 1/27 on this bench — not because the theory was
wrong but because kaos was BROKEN on this provider:

1. **Seed overflow (100% call failure).** kaos sent the spiral's u64 seed raw in
   the request body; Phala (and other OpenRouter providers) reject a seed
   outside non-negative i32 with an HTTP 400 — so EVERY call failed, the model
   never responded, the spiral fizzled. The ollama path already clamped; the
   OpenRouter/native paths did not. Fixed: clamp `seed % i32::MAX` on both
   (src/provider.rs). NOTE: any earlier kaos measurement that routed to a
   seed-capped provider was silently degraded by this — a confound worth
   remembering when re-reading older nulls.

2. **Ambiguous-anchor edit corruption.** edit_file replaced the FIRST match of
   `find`; a weak model passing a tiny anchor (a bare identifier) silently
   edited the wrong place and corrupted the file past recovery (the syntax-lint
   veto doesn't catch semantic breakage like a clobbered import). Fixed: refuse
   when `find` matches >1 place and demand unambiguous context (src/conductor.rs),
   matching standard edit-tool semantics. 150 tests still green.

**What this does and does NOT claim.** It does NOT overturn the finding above:
kaos still has no demonstrated edge over opencode on CAPABLE models. It DOES,
for the first time, measure kaos beating opencode head-to-head in the exact
regime the theory predicted — a weak model in its mid-band, strong gate — and
substantially (2.5x). One model, one bench (9 bugs); a real, reproducible,
pre-registered datapoint, not a broad claim. The kept nulls stand; this is a
kept positive, bounded to its regime.

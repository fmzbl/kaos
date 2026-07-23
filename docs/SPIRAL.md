# The Spiral — why Fibonacci is load-bearing, not liturgical

Everything the engine holds or spends is a sigil: the intent, every
observation, every attempt, every sample. Sigils carry charge; charge is
finite; and the question a coding agent answers a hundred times a session is
always the same one — **which sigil gets the next unit of energy?**

This document states the claim precisely, gives the mathematics that backs
it, and lists the falsifiable predictions. The naming layer is Carroll; the
skeleton is restart theory and heavy-tailed distributions.

## The observation that started it

The ledgerbench data (`docs/EDGE.md` records the runs) showed agent solve
times on same-difficulty bugs spanning **8 s to 600 s** — a heavy-tailed
distribution. A run either converges quickly or wanders, and the wandering
runs rarely recover, because they wander atop their own accumulating context
rot. One instance died with 40 steps spent and **zero edits made**; another
trace showed the model re-reading one unchanged file **eight consecutive
turns**.

Heavy tails change what the optimal policy is. Against a heavy-tailed
solve-time distribution, the theory of restarts (Luby, Sinclair & Zuckerman
1993; the restart schedules inside every modern SAT solver) says:

1. **Many short attempts with fresh state beat one long run.** The tail is
   where the expected cost lives; cutting a wandering run and starting fresh
   re-rolls the draw.
2. **The attempt budgets should grow geometrically**, with a ratio between
   1.5 and 2 — long enough that hard instances eventually get a real budget,
   short enough that the early cheap probes dominate the spend.
3. **The attempts must be independent draws**, or restarting just replays
   the same failure.

φ = 1.618… sits inside the optimal ratio window, and the Fibonacci numbers
are φ's integer schedule — consecutive ratios 8/5, 13/8, 21/13 converge on φ.
**Fibonacci is what a geometric restart schedule looks like when it must be
made of whole steps.** That is the sense in which the fib idea is the core of
the engine and not decoration on it.

## The three mechanisms

### 1. The Spiral (`src/spiral.rs`) — fib restart scheduling

Attempt step-budgets follow 5, 8, 13, 21, … capped so their sum never
exceeds what one long run would have spent: the spiral **redistributes** the
budget, it does not grow it. A failed working is **banished** — Carroll's
reset after a failed operation — meaning its context is discarded whole and
only a distilled verdict crosses the gap ("the gate said X", "the attempt
changed no file").

With a gate, failure is the gate's verdict. Without one, failure is still
observable: the **fizzle** — a session that errors, exhausts its steps, or
"finishes" having changed nothing. The zero-diff loss in the benchmark data
is exactly a fizzle the old engine had no answer to.

### 2. Polarity — the two universes

Restart theory needs independent draws. Hosted endpoints don't reliably
honour seeds, but they honour temperature — so consecutive attempts
alternate between two universes of sampling, each the other's reverse twin
around the midpoint:

- **solar** — temperature 0.35: cold, convergent, precise;
- **lunar** — temperature 0.85: hot, divergent, exploratory.

A banished working is never retried under the same stars. The first attempt
is solar (most bugs want precision); its twin explores where precision
failed.

### 3. The Twin Ladders (`src/charge.rs`) — fib context decay

Within one attempt, the transcript is a tunnel of sigils and charge decays
by Fibonacci from both mouths: the **intent** (never compressed — the most
charged sigil there is) and the **freshest observation** (fib 8·5·3·2·1,
which is φ-decay in whole numbers). The middle holds base charge and is
allowed to rot. Polarity appears here too: a positive sigil cut to budget
keeps its head, a negative one keeps its tail — where tracebacks put the
punchline. See `docs/twin-ladders.html` for the rendered tunnel.

The ladders and the spiral are the same idea at two scales: **charge decays
by φ across context-distance within an attempt, and grows by φ across
attempt-number within a session.** Two universes, mutually reversed — the
descending twin and the ascending; the solar draw and the lunar.

## What this predicts (falsifiable)

1. **The zero-diff failure mode disappears.** A fizzled 5-step probe costs 5
   steps, not 40; the spiral re-rolls it. (lb-004 class.)
2. **Tail wall-times collapse.** Sessions like the 327 s / 600 s outliers get
   cut at their fib boundary and usually resolve in a later, fresher attempt;
   mean wall-time drops even though attempt count rises.
3. **Fast solves stay fast.** An 8 s solve fits inside the first 5-step
   budget; the spiral costs nothing when the model is right immediately.
4. **Solar-then-lunar beats solar-then-solar** on the instances where attempt
   1 fails — measurably, because the second draw is actually different.

If ledgerbench (v1 + the Second Veil in v2) does not show (1) and (2)
against the pre-spiral records, the schedule loses its place in the engine.
The eight-pointed correspondence with the rays keeps the naming either way;
the mechanism only stays if the gate says so.

## Honest limits

- The optimal-restart theorems assume you can't observe progress mid-run;
  an agent partially can (edits made, tests closer). The spiral already uses
  the crudest such signal (the fizzle); finer ones (gate-distance) would
  beat blind scheduling and should eventually replace it.
- φ being *inside* the optimal ratio window is not the same as φ being
  *optimal*; ratio 2 (5, 10, 20, …) might measure the same. The fib choice
  is φ-principled and on-brand, but the ablation (5,8,13,21 vs 5,10,20,40)
  is cheap and should be run.
- On models strong enough to one-shot everything, none of this matters —
  the first 5-step budget absorbs the whole distribution and the spiral
  never turns. The edge, as always in this repo, lives in the mid-band.

## Neural counterpart: Sisyphus

[Sisyphus](SISYPHUS.md) applies the same bounded recurrence at two model scales.
Inside a forward pass, one quote/route/group/square cell is reused over dyadic
context scales and two refinement rounds; a `1/sqrt(visits)` residual keeps
tied depth stable, and later rounds are trained not to worsen earlier
predictions. Outside the model, candidate checkpoints inherit the champion,
train under Fibonacci attempt budgets, alternate cold/hot optimizer polarity,
and cross a frozen validation gate or roll back exactly.

This is the safe reading of recursive self-improvement: real evidence enters at
the gate, failed state is banished, and the objective cannot rewrite its own
judge. Round two improved round one in all ten retained enwik8/text8 seeds. The
final model beat a matched Transformer 4/5 on enwik8, but lost 0/5 on the
untouched text8 confirmation. Recurrence therefore improves Sisyphus's own
intermediate state reliably without establishing general superiority; the
[paper](../sisyphus/PAPER.md) retains the complete limits.

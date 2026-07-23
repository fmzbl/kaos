# Sisyphus: an enwik8 positive, a failed text8 confirmation, and a long-context edge

## Abstract

Thoth demonstrated that a causal counter-recursive language model could be
implemented, grown, compressed, and served, but its own controlled benchmark
was negative: a matched Transformer was both more accurate and roughly five
times faster on Tiny Shakespeare. We ask whether the architectural idea can be
reduced to the semantics of Rebis, a small executable model-interface language.

We introduce **Sisyphus**, a decoder-only raw-byte language model with four
soft neural operations—quote, route, group, and square mediation—reused over
dyadic causal scales and bounded recursive refinement rounds. Stored parameters
are effectively independent of context length, while sequence mixing costs
`O(R n log n)` rather than attention's `O(n²)`. Later rounds are deeply
supervised and penalized when they worsen earlier next-byte predictions.

In a locally pre-specified five-seed enwik8 micro-study, Sisyphus (29,811
parameters) reached mean held-out 3.7491 bits/byte versus 3.8013 for a modern
causal Transformer (29,792 parameters) under identical 256,000-byte training
schedules. Sisyphus won four of five paired seeds; mean paired difference was
−0.0522 bpb and the paired bootstrap 95% interval was [−0.0971, −0.0061]. Its
second recursive pass improved the first on all five seeds. Because seed-0
enwik8 test scores had been observed during architecture calibration, we then
froze every setting and ran a no-pilot, five-seed text8 confirmation. It failed:
Sisyphus lost all five pairs, 3.1493 versus 3.0679 mean bpb, with paired 95%
interval `[+0.0439, +0.1248]` in the Transformer's favor. At context 128, the
control trained about five times faster on both corpora. Isolated CPU inference
crossed over at context 2,048 among tested lengths; at 4,096, Sisyphus was
5.04× faster and used 83.3% less peak resident memory. The evidence supports an
enwik8-specific sample-efficiency result, reliable internal revision, and a
long-context runtime edge—not a general language-quality or state-of-the-art
claim.

## 1. Motivation

The original Thoth decoder realizes a rich direct/detour square law: prograde
hierarchies write persistent mediator slots, an exclusive retrograde prefix
descends through completed squares, and returned evidence revises both leaf
state and square memory. This is causally coherent and testable. It is also
mechanically expensive. Thoth's retained official study reports mean 3.2206
bpb against 2.9150 for a matched Transformer, with about one fifth of the
Transformer's training throughput.

Rebis suggests a smaller basis. Its useful computational semantics are not its
punctuation but four operations:

- quoted syntax remains inert until deliberately used;
- arrows route accepted values forward;
- groups compose values in order;
- executable squares mediate competing branches.

Sisyphus asks whether these operations can be a parameter-shared neural mixer,
retaining recursive computation and square mediation without reproducing
Thoth's entire tree of specialized write/read/revision networks.

The name is literal engineering: each round pushes the same state through the
same hill of scales. Unlike the myth, the state should return better. The
benchmark—not the name—decides whether it did.

## 2. Architecture

Let `h^(r,l)` be the token states at refinement round `r` and dyadic scale `l`.
For offset `s_l = 2^l`, define a zero-padded causal shift `S_l(h)_i = h_(i-s_l)`
when `i >= s_l`, else zero. Every output at position `i` therefore depends only
on positions at or before `i`.

After RMS normalization, one shared cell forms local and routed currents:

```text
a = W_local norm(h) + e_level[l] + e_round[r]
b = W_route S_l(norm(h))
```

It constructs four Rebis candidates:

```text
quote   q = norm(h)
route   t = b
group   g = (a + b) / 2
square  m = tanh(W_mediator [a; b])
```

An operator router chooses their convex mixture:

```text
p = softmax(W_operator [a; b])
u = W_output sum_k p_k candidate_k
h <- h + u / sqrt(rounds * active_scales)
```

After all active scales, a shared SwiGLU residual completes the round. The
official model uses width 41, hidden width 82, one physical block, two rounds,
and tied byte embeddings. Fixed 0.02-amplitude sinusoidal positions are buffers,
not parameters. Each context doubling adds only one learned width-41 scale
code: physical parameters grow logarithmically from 29,729 at context 32 to
30,016 at context 4,096.

### 2.1 Recursive revision objective

The language metric is pure final-round next-byte negative log likelihood.
Training adds two auxiliary terms:

```text
L = L_final + 0.15 mean(L_intermediate)
              + 0.05 mean(relu(L_after - L_before))
```

The last term directly discourages a later recursive pass from worsening the
targeted prediction. It does not let the model see future input tokens; targets
are used only by the training loss, as in ordinary teacher forcing.

### 2.2 Complexity

With fixed width and rounds, sequence mixing performs `log2(n)` linear passes:

- Sisyphus mixer time: `O(R n log n d²)` in this dense reference cell;
- Sisyphus activation memory: `O(n d)` aside from compiler temporaries;
- full attention score time/memory: quadratic in `n`.

The reference implementation is not a fused kernel. Short contexts are
therefore dominated by repeated linear-map and launch overhead even where the
asymptotic count is favorable.

## 3. Grounded recursive improvement

Internal rounds revise activations. A separate loop may revise weights, but it
uses the same boundary Kaos and Rebis impose on `evolve`: an external gate keeps
the better artifact.

For attempt `j`, `improve.py`:

1. loads an immutable champion checkpoint;
2. creates a candidate from exactly that champion;
3. trains on the real training split under Fibonacci budgets `5, 8, 13, 21, …`;
4. alternates a cold/hot learning-rate multiplier for restart diversity;
5. evaluates both on the same frozen validation windows;
6. atomically promotes only when candidate bpb clears a declared margin;
7. retains candidate, decision, schedule hash, scores, and checkpoint hashes.

A rejected attempt leaves the champion SHA-256 unchanged. The test split is not
part of the loop. Self-generated samples are excluded: Thoth's earlier recursive
self-training experiment showed the ungrounded arm collapsing, and even its
gated arm degraded substantially. Sisyphus calls continual learning
"self-improvement" only when new real evidence crosses a frozen gate.

## 4. Experimental protocol

The complete frozen local protocol is [`PROTOCOL.md`](PROTOCOL.md).

### 4.1 Data and decision

We use the exact 100,000,000-byte enwik8 corpus (uncompressed SHA-256
`2b4972…024a8`) with contiguous 90/5/5 train/validation/test splits and a
256-byte vocabulary. Five fixed seeds receive byte-identical paired batch-start
matrices. Each arm trains for 500 AdamW updates, batch 4, context 128: 256,000
training bytes. Validation uses 32 fixed evenly spaced windows. Test uses 64
fixed windows once at the final checkpoint.

An edge requires all five pairs, at least four Sisyphus wins, and a deterministic
paired-bootstrap 95% interval wholly below zero. Seed-0 architecture and length
pilots are excluded from the five-seed estimate, but their enwik8 test scores
were observed during calibration. The enwik8 result is therefore an internal
multi-seed positive, not a fully untouched corpus confirmation.

Before any second-corpus run, we froze the same architecture, optimizer, byte
budget, and rule in [`CONFIRMATION.md`](CONFIRMATION.md), downloaded text8 for
the first time, and used five new seeds `19, 31, 47, 73, 127`. No text8 pilot or
inter-pair tuning occurred.

### 4.2 Control

The control is a two-block width-32 causal Transformer with four-head RoPE
attention, pre-RMSNorm, width-69 SwiGLU, tied byte embeddings, and no biases in
the main projections. At context 128 it has 29,792 parameters, 19 fewer than
Sisyphus (0.064%). Optimizer, schedules, training bytes, splits, and evaluation
windows are identical.

### 4.3 Root-project artifact

The complete implementation is a first-class root package at `sisyphus/`,
beside Kaos's Rust `src/` runtime rather than nested under an auxiliary research
tree. `python -m sisyphus` provides one command surface for the benchmark,
summary, scaling study, and gated improvement loop. Model code, complete run
records, final checkpoints, checkpoint digests, the frozen protocol, and the
failed confirmation are retained together. Moving the package did not alter
the four implementation files hashed into the official records; integrity
tests recompute those hashes and each paired batch-schedule identity. A
package-local compiler bridge uses the pinned ziglang dependency when clang is
not installed, eliminating the former checkout-layout dependency on Thoth.

This repository integration does not turn the 30k-parameter checkpoint into a
Kaos inference provider. Training remains in an optional tinygrad environment,
while the Rust application uses its existing provider seams. That operational
boundary keeps research-scale evidence distinct from production capability.

## 5. Results

### 5.1 Enwik8 held-out language modeling

| Seed | Sisyphus bpb | Transformer bpb | Difference |
|---:|---:|---:|---:|
| 17 | 3.7448 | 3.7545 | −0.0098 |
| 29 | 3.7088 | 3.8215 | −0.1128 |
| 43 | 3.7270 | 3.7940 | −0.0670 |
| 71 | 3.8255 | 3.8003 | +0.0252 |
| 113 | 3.7397 | 3.8361 | −0.0964 |
| **Mean** | **3.7491** | **3.8013** | **−0.0522** |

Sisyphus wins four pairs and reduces mean bpb by 1.37%. The paired bootstrap
95% interval is `[−0.0971, −0.0061]`; the frozen narrow quality rule passes.

The result is also variable: one seed loses, the smallest win is only 0.0098
bpb, and the interval approaches zero. The untouched cross-corpus confirmation
below is therefore the stronger test of generality.

### 5.2 Untouched text8 confirmation

| Seed | Sisyphus bpb | Transformer bpb | Difference |
|---:|---:|---:|---:|
| 19 | 3.1545 | 3.0581 | +0.0963 |
| 31 | 3.1925 | 3.0302 | +0.1623 |
| 47 | 3.1460 | 3.0687 | +0.0773 |
| 73 | 3.1343 | 3.0938 | +0.0405 |
| 127 | 3.1191 | 3.0885 | +0.0305 |
| **Mean** | **3.1493** | **3.0679** | **+0.0814** |

The Transformer wins all five pairs. The paired bootstrap 95% interval is
`[+0.0439, +0.1248]` bpb, wholly against Sisyphus; its mean bpb is 2.65% worse.
The identical model and budget therefore do not have a general short-context
quality edge. A plausible inference is that Sisyphus's dyadic mixer benefits
more from enwik8's raw markup and byte diversity than from cleaned lowercase
text, but this mechanism has not been isolated and is not claimed as fact.

### 5.3 Did recursion improve itself?

Round-two minus round-one test bpb by seed:

```text
−0.0520, −0.0686, −0.0666, −0.0718, −0.0636
```

All five enwik8 later passes improve; mean change is −0.0645 bpb (standard error
0.0034). All five text8 later passes also improve, by −0.0363 bpb on average.
This supports the mechanistic revision claim within the trained model while
showing that internal improvement does not imply superiority to a control.

### 5.4 Efficiency

At the trained context of 128 on enwik8, Sisyphus averages 1,025 training
bytes/s versus 5,213 for the Transformer; on text8, 1,039 versus 5,361. The
control is 5.09–5.16× faster. Enwik8 sample efficiency and wall-clock efficiency
point in opposite directions; text8 favors the control on both.

Isolated batch-one CPU inference, median of ten post-compilation runs:

| Context | Sisyphus tok/s | Transformer tok/s | Sisyphus speed | Peak RSS reduction |
|---:|---:|---:|---:|---:|
| 32 | 7,338 | 8,762 | 0.84× | −2.3% |
| 128 | 3,990 | 11,228 | 0.36× | −6.8% |
| 512 | 11,155 | 16,831 | 0.66× | −1.4% |
| 1,024 | 17,251 | 20,139 | 0.86× | 18.4% |
| 2,048 | 18,586 | 11,566 | 1.61× | 55.3% |
| 4,096 | 30,920 | 6,140 | 5.04× | 83.3% |

The measured crossover among tested lengths is 2,048. The non-monotonic short
results reflect compiler/kernel behavior; the long trend matches the expected
`n log n` versus `n²` structural work.

## 6. Relationship to prior work

Sisyphus belongs to the broad family of recurrent-depth and efficient sequence
models, but its operator basis and dyadic causal routing come specifically from
the Rebis/Kaos/Thoth lineage. Looped models show that depth can be decoupled
from stored parameters; recent work also emphasizes that tied-depth residual
scaling differs from ordinary untied depth. Sisyphus uses a conservative
`1/sqrt(visits)` residual factor for this reason. Test-time-memory systems such
as Titans motivate persistent adaptation, while Sisyphus deliberately keeps
weight promotion outside inference and behind held-out evidence.

Primary references:

- Tay et al., [Long Range Arena](https://openreview.net/forum?id=qVyeW-grC2k),
  ICLR 2021.
- Behrouz et al., [Titans: Learning to Memorize at Test Time](https://arxiv.org/abs/2501.00663),
  2024.
- Li et al., [DeepLoop: Depth Scaling for Looped Transformers](https://arxiv.org/abs/2607.13491),
  2026.
- Hutter Prize, [contest FAQ and compression/intelligence rationale](https://www.hutter1.net/prize/hfaq.htm).
- Thoth, [counter-recursive research and runtime](https://github.com/fmzbl/ra).
- Rebis, [model-interface language](https://github.com/fmzbl/rebis).

## 7. Limitations and next falsifiers

1. **Scale:** 30k parameters and 256k training bytes are a micro-study, not a
   useful general language model.
2. **Quality does not transfer:** the untouched text8 confirmation favors the
   Transformer 5/5. The enwik8 positive is dataset-specific on current evidence.
3. **Five seeds per corpus:** both intervals are informative but still small.
   New corpora and seeds matter more than reusing either retained test split.
4. **Short-context cost:** the reference recursive loop is substantially slower
   during context-128 training. Fusion or a parallel associative scan is needed.
5. **Unequal operations:** matching parameters and bytes does not match FLOPs or
   wall time. Iso-FLOP and iso-wall-clock curves may favor the Transformer.
6. **No reasoning claim:** next-byte compression does not establish tool use,
   program synthesis, or recursive problem solving. LRA ListOps or a similarly
   fixed structural benchmark should test that separately.
7. **No autonomous-objective claim:** the improvement loop cannot rewrite its
   gate, corpus split, margin, or protocol. That constraint is a feature.

The next result that would materially strengthen the paper is a predeclared
multi-scale study on new corpora: 30k/300k/3M parameter regimes, plus fixed-token,
fixed-FLOP, and fixed-wall-clock comparisons. Today the defensible conclusion
is already clear: Sisyphus is an interesting recursive long-context mixer with
a real enwik8 positive and a real text8 failure, not a generally better language
model.

## 8. Iteration zero of the AGI-foundation loop: registered unverified hypotheses

This section was added on 2026-07-19 at the start of a bounded research loop
that extends Sisyphus toward two new ideas. Neither idea has any implementation
or evidence in this repository yet. Both are registered here as falsifiable
hypotheses so that later iterations are judged against a record written before
any code existed. Everything above this section is retained unchanged.

### 8.1 Reconstruction audit of the existing evidence (2026-07-19)

Before registering anything new, the retained evidence was re-verified in
place:

- all 12 unit tests pass (`python -m unittest discover -s sisyphus -t .`),
  including `test_results.py`, which recomputes the enwik8 decision rule, the
  text8 failure, the paired batch-schedule identities, and the scaling
  crossover from the retained JSON records;
- every checkpoint in `results/enwik8_v1/checkpoints.sha256` and
  `results/text8_v1/checkpoints.sha256` verifies with `sha256sum -c`;
- the retained `summary.json` files reproduce every number in Sections 5.1,
  5.2, 5.3, and 5.4, including the paired bootstrap intervals
  `[-0.0971, -0.0061]` (enwik8) and `[+0.0439, +0.1248]` (text8).

The quote/route/group/square cell described in Section 2 is implemented as
`RebisCell` in `models.py` (operators declared at `models.py:68`, mediation and
operator softmax at `models.py:106-109`), with causality, parameter matching,
and finite-loss tests in `test_models.py`. The gated improvement loop of
Section 3 is `improve.py:129`. This audit is the baseline any new architecture
must preserve: the existing protocol, records, and tests are frozen inputs,
not editable material.

### 8.2 Hypothesis H1 (unverified): the Complex Rebis Path Machine

**Claim.** A causal cell whose quote/route/group/square operators act on a
complex-valued hidden state, and whose recursive rounds form a learned
iterative path `z_0, z_1, …, z_T` through that complex space, improves held-out
prediction or compositional capability over the current real-valued Sisyphus
cell at matched parameters and training bytes.

**Novelty burden.** Complex-valued RNNs, unitary RNNs, and complex state-space
models are established. H1 is only interesting if each operator is derived
from an executable Rebis semantic (inert quote, causal route with an explicit
phase, order-preserving group, mediating square) and if the *path* through
rounds — phase, radius, winding, interference — does measurable work. A model
that merely swaps dtypes is a null result by definition, not a partial success.

**Smallest disproving experiment.** A five-seed paired study under the exact
`sisyphus-byte-v1` budget (500 updates, batch 4, context 128, 256,000 bytes)
comparing four matched-parameter arms: (a) the complex path machine, (b) the
identical recurrence with the complex state replaced by a real state of equal
parameter count, (c) the complex machine with rounds reduced to one
(non-recursive), and (d) the complex machine with phases randomized at
readout (phase-destroyed). Design and seed selection use only training and
validation windows; the frozen test splits of Sections 5.1–5.2 are consulted
once, at the end, under the Section 4.1 decision rule. H1 is **falsified for
this scale** if (a) fails to beat (b) under the paired-bootstrap rule, and the
path claim specifically is falsified if (a) fails to beat (c) and (d).
Status: TODO — no complex implementation exists in this repository yet.

### 8.3 Hypothesis H2 (unverified): gated musical feedback helps the main model

**Claim.** A separately optimized side model, trained on frozen versioned
exports of the main model (hidden states, logits, or operator traces), can
compose symbolic music with explicit synthesizer controls and return a
compact, auditable feedback packet whose gated, adapter-bounded application
improves a predeclared main-model target metric without violating non-music
regression or anti-forgetting thresholds.

**Constraints carried forward.** The sidecar is opt-in and disabled by
default; the teacher export is immutable at a checkpoint boundary; optimizer
and checkpoint state are separate; no sidecar-to-main weight writes occur
outside the gate; artifacts, seeds, provenance, and accepted/rejected update
records are retained; no network service, microphone capture, or unbounded
background process exists. Self-generated music is never its own ground truth,
matching the Section 3 rule that ungrounded self-training is known to
collapse.

**Smallest disproving experiment.** One complete bounded cycle: export a
frozen teacher snapshot, train the sidecar, render deterministic symbolic
artifacts, and evaluate one candidate feedback update against three matched
controls — no feedback, shuffled teacher signals, and an unconditioned
sidecar. H2 is **falsified for this scale** if the conditioned sidecar's
held-out musical metrics fail to beat the shuffled-teacher control (the
teacher signal carries no usable information), or if the gated update fails
its predeclared non-music regression bound (feedback harms the main model),
or if the update's target-metric gain does not exceed its declared noise
threshold. A rejected update leaves the main checkpoint byte-for-byte
unchanged and is itself retained as evidence.
Status: TODO — no sidecar code, artifacts, or feedback records exist yet.

### 8.4 What iteration zero does not claim

No complex-valued result, no musical result, and no capability beyond
Sections 5.1–5.4 exists at the time of this registration. Nothing in this
loop, at any iteration, licenses an AGI claim from toy language-model or
music results; the stopping rule of the loop is a usable, audited research
foundation, not a proclamation.

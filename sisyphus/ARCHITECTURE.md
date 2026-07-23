# Sisyphus architecture

Sisyphus is now a first-class component at the Kaos repository root. It is a
causal byte-language model whose sequence mixer is built from the semantic
operations of Rebis rather than multi-head self-attention.

```text
bytes + fixed positions
          |
          v
  quote / route / group / square
          |  shared over dyadic causal scales
          v
  recursive refinement round 1
          |
          v
  recursive refinement round 2
          |
          v
 tied byte projection -> next-byte distribution
```

## Neural operation basis

- **Quote** retains a normalized local representation.
- **Route** shifts evidence from an earlier causal location.
- **Group** composes local and routed currents.
- **Square** mediates the direct and routed representations with a learned map.

A learned operator distribution mixes those four candidates at every dyadic
offset `1, 2, 4, ...`. The same cell is reused across offsets and refinement
rounds. With fixed width and rounds, sequence mixing requires `O(n log n)`
work and `O(n)` activation storage instead of constructing an `n x n`
attention matrix.

## Bounded recursion

The two internal rounds are deeply supervised. Training penalizes later
rounds when they worsen the earlier next-byte loss. Across the ten retained
enwik8 and text8 seeds, the second round improved the first every time. This is
evidence for useful internal revision, not evidence that the model can safely
rewrite its own goals.

External weight improvement is implemented separately. Each candidate inherits
an immutable champion, trains only on real training bytes, and must improve a
fixed validation gate by a declared margin. Promotion is atomic; rejection
leaves the champion checkpoint unchanged.

## Repository boundary

The root package owns architecture, training, evaluation, retained checkpoints,
and the paper:

```text
sisyphus/
  __main__.py               unified root command surface
  models.py                 neural architecture and Transformer control
  compiler.py + zigcc       self-contained tinygrad CPU compiler selection
  improve.py                promotion-gated continual learning
  benchmark_lm.py           paired fixed-token language benchmark
  benchmark_scaling.py      isolated context-scaling benchmark
  results/                  immutable run records and checkpoints
  PROTOCOL.md               enwik8 protocol and claim boundary
  CONFIRMATION.md           untouched text8 falsification
  PAPER.md                  complete research report
```

Kaos's Rust application and Rebis runtime remain operationally independent of
the Python training stack. That boundary prevents a 30k-parameter research
checkpoint from being mistaken for a production provider. The root placement
makes Sisyphus part of the project architecture; provider integration still
requires a scaled checkpoint and a stable inference service.

## Evidence boundary

Sisyphus beats the matched Transformer on the retained enwik8 study but loses
all five pairs on the untouched text8 confirmation. It therefore has no
demonstrated general short-context quality advantage. Its strongest replicated
architectural properties are useful recursive revision and the measured
long-context CPU throughput/memory crossover. Exact results and limitations
are in [PAPER.md](PAPER.md).

## Iteration one: the Complex Rebis Path Machine and the music sidecar

Two additive, opt-in extensions were registered as unverified hypotheses in
`PAPER.md` section 8 and then implemented:

- **`complex_path.py`** (hypothesis H1): a quote/route/group/square cell whose
  hidden state is a complex number stored as a real `(re, imag)` pair, whose
  causal `route` carries an explicit learned phase `exp(i * theta_level)`, and
  whose recursive rounds form a path `z_0, ..., z_T` through that complex
  space. `build_path_model` constructs four matched-parameter arms --
  `complex`, `real` (the block-diagonal, non-complex ablation of the exact
  same weight matrices), `nonrecursive` (rounds forced to 1), and
  `phase-destroyed` (the readout is rotated by an independent random phase) --
  used by `benchmark_complex_path.py` and `summarize_complex_path.py` to run
  and judge the pre-registered H1 falsification rule. It does not touch
  `models.py`, and none of the retained enwik8/text8 evidence changed.
- **`music/` and `music_sidecar.py`** (hypothesis H2): an opt-in sidecar that
  exports a frozen, versioned snapshot of a main model's hidden states,
  operator traces, and phase/radius diagnostics (`music/teacher.py`), maps
  those frozen features through a fixed, non-learned rule into a symbolic
  target (`music/target.py` -- the deterministic answer key that keeps the
  sidecar's supervision grounded instead of self-certifying), trains a small
  separately optimized student to predict that target from the teacher vector
  (`music/sidecar_model.py`), renders deterministic MIDI and WAV artifacts
  (`music/midi.py`, `music/synth.py`), computes a compact feedback packet
  (`music/feedback.py`), and applies it to the main model only through a
  rank-limited adapter behind a held-out, replay-gated, checkpoint/rollback
  update (`music/adapter.py`). The CLI (`music_sidecar.py`) requires an
  explicit `--enable` flag; there is no daemon, network service, or
  microphone capture anywhere in the package.

Both extensions are pilot-scale falsification studies, not production
architecture changes. Measured results, what is genuinely novel versus
relabeled, and what remains a TODO are in `PAPER.md` section 9.

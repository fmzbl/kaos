# Sisyphus

Sisyphus is an experimental causal language model whose neural operations are
drawn from Rebis: retain quoted state, route earlier evidence, group currents,
and reconcile them through a square mediator. One cell is reused across
power-of-two causal scales and recursive refinement rounds. It reimagines
Thoth's counter-recursive decoder as a smaller, simpler `O(n log n)` mixer.

This root directory is Kaos's first-class neural architecture component. It
owns the model, training loop, benchmark runners, retained evidence, and paper.
Its current checkpoint remains research-scale rather than a production
inference backend, and the measured claim is deliberately narrow:

> In a five-seed, 29.8k-parameter, 256k-training-byte enwik8 micro-study,
> Sisyphus reduced mean held-out loss from 3.8013 to 3.7491 bits/byte versus a
> matched modern Transformer. It won 4/5 pairs; the paired bootstrap 95%
> interval for Sisyphus minus Transformer was `[-0.0971, -0.0061]` bpb.

That is a 1.37% enwik8 bpb reduction at equal training bytes and nearly equal
parameters. It did **not** generalize to the untouched text8 confirmation:
Sisyphus lost 0/5 pairs, 3.1493 versus 3.0679 mean bpb, with paired 95% interval
`[+0.0439, +0.1248]` in the Transformer's favor. There is no demonstrated
general short-context quality edge. The retained edges are enwik8-specific
sample efficiency and independently measured long-context inference
efficiency; neither is a state-of-the-art claim.

## What is here

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — operation basis and project boundary.
- [`__main__.py`](__main__.py) — unified `python -m sisyphus` command surface.
- [`models.py`](models.py) — Sisyphus and the matched Llama-style Transformer.
- [`benchmark_lm.py`](benchmark_lm.py) — resumable paired raw-byte LM study.
- [`PROTOCOL.md`](PROTOCOL.md) — frozen decision rule and exact official command.
- [`results/enwik8_v1/summary.json`](results/enwik8_v1/summary.json) — official
  five-seed enwik8 positive.
- [`CONFIRMATION.md`](CONFIRMATION.md) and
  [`results/text8_v1/summary.json`](results/text8_v1/summary.json) — frozen,
  no-pilot cross-corpus failure.
- [`benchmark_scaling.py`](benchmark_scaling.py) — isolated context-scaling runs.
- [`results/scaling_v1/summary.json`](results/scaling_v1/summary.json) — measured
  CPU inference crossover.
- [`improve.py`](improve.py) — promotion-gated continual improvement with exact
  rollback and a persistent audit ledger.
- [`PAPER.md`](PAPER.md) — architecture, experiment, results, and claim boundary.

## Architecture in one pass

For each refinement round and dyadic offset `1, 2, 4, ...`, a causal cell forms
four candidate operations:

1. `quote`: retain the local normalized state;
2. `route`: read a state shifted from the causal past;
3. `group`: compose local and routed currents;
4. `square`: reconcile direct and detour evidence through a learned mediator.

A softmax chooses a mixture, and a depth-scaled residual commits it. The same
weights are reused at every scale and round. At context 128, Sisyphus has
29,811 parameters versus 29,792 for the control. At context 4,096 it still has
only 30,016 because positions are fixed and each doubling adds one 41-value
scale code.

The second round is not decorative: it improved first-round held-out bpb in all
ten enwik8/text8 seeds—by 0.0645 and 0.0363 bpb on average, respectively. On
text8, that internal gain was not enough to beat the Transformer.

## Reproduce

Create a Python 3.11+ environment with:

```bash
python -m venv .venv-sisyphus
.venv-sisyphus/bin/pip install -r sisyphus/requirements.txt
PYTHONPATH=. DEV=CPU .venv-sisyphus/bin/python -m sisyphus --help
```

The requirements include tinygrad's ziglang fallback, and the package-local
`zigcc` bridge selects it automatically when system clang is unavailable.

Download and verify enwik8:

```bash
curl -L --fail -o /tmp/enwik8.zip https://mattmahoney.net/dc/enwik8.zip
unzip /tmp/enwik8.zip -d /tmp/sisyphus-data
sha256sum /tmp/sisyphus-data/enwik8
# 2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8
```

Run the enwik8 protocol with the command in [`PROTOCOL.md`](PROTOCOL.md); the
untouched text8 confirmation and its exact command are in
[`CONFIRMATION.md`](CONFIRMATION.md).
Run the focused tests with:

```bash
PYTHONPATH=. DEV=CPU python -m unittest discover -s sisyphus -t . -v
```

## Bounded continual improvement

`improve.py` is the external recursive loop. A candidate inherits the current
champion, trains on real training bytes, and can replace the champion only when
a frozen validation gate improves by the declared margin. Attempts use
Fibonacci budgets and alternate low/high learning-rate polarity. Candidates,
decisions, schedules, scores, and checkpoint hashes are retained.

```bash
PYTHONPATH=. DEV=CPU python -m sisyphus improve \
  --corpus /path/to/owned-corpus \
  --state-dir /path/to/sisyphus-state \
  --attempts 4
```

The test split is never consulted, rejected candidates cannot mutate the
champion, and model-generated text is never treated as its own ground truth.
This is gated continual learning—not unconstrained recursive self-modification.

## Efficiency result

At context 128, the tinygrad CPU Transformer trains 5.09× faster on enwik8 and
5.16× faster on text8. Sisyphus has an enwik8 sample-efficiency edge there, but
not a general quality or wall-clock edge. Isolated inference crosses over among
the measured contexts at 2,048 bytes:

| Context | Sisyphus speed / Transformer | Peak RSS change |
|---:|---:|---:|
| 1,024 | 0.86× | −18.4% |
| 2,048 | 1.61× | −55.3% |
| 4,096 | 5.04× | −83.3% |

These are backend- and hardware-specific measurements, not universal kernel
claims.

## Iteration one: complex path machine and music sidecar (opt-in, pilot-scale)

Two additive falsification studies extend Sisyphus without touching the
evidence above; see [ARCHITECTURE.md](ARCHITECTURE.md#iteration-one-the-complex-rebis-path-machine-and-the-music-sidecar)
and [PAPER.md](PAPER.md) section 9 for the full account.

```bash
# H1: the four-arm complex/real/nonrecursive/phase-destroyed pilot
PYTHONPATH=. DEV=CPU python -m sisyphus complex-path-pilot \
  --output-dir /tmp/sisyphus-complex-path-pilot \
  --corpus-path /tmp/sisyphus-synthetic-corpus.bin
PYTHONPATH=. DEV=CPU python -m sisyphus summarize-complex-path \
  /tmp/sisyphus-complex-path-pilot

# H2: one bounded, opt-in music-sidecar cycle (disabled unless --enable is passed)
PYTHONPATH=. DEV=CPU python -m sisyphus music-sidecar \
  --enable \
  --artifact-dir /tmp/sisyphus-music-sidecar \
  --corpus-path /tmp/sisyphus-synthetic-corpus.bin
```

Both commands run new tests in `test_complex_path.py` and
`test_music_sidecar.py`, included in the same `unittest discover` run above.

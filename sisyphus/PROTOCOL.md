# Sisyphus–Transformer enwik8 micro-study

Protocol version: `sisyphus-byte-v1`  
Status: development protocol; seed-0 pilots are calibration only

This protocol was written after exploratory seed-0 runs and before any of the
five official paired seeds. It is local and uncommitted, so it is not an
independent preregistration. Pilot test scores informed architecture debugging
and are excluded from inferential evidence.

## Question and decision rule

At approximately equal physical parameter count and exactly equal training
bytes, does Sisyphus improve held-out raw-byte prediction over a modern causal
Transformer on enwik8?

The primary estimand is paired final test bits per byte (Sisyphus minus
Transformer). Lower is better. A narrow quality edge is supported only if:

1. all five fixed paired seeds `17, 29, 43, 71, 113` finish;
2. Sisyphus wins at least four pairs; and
3. a deterministic paired bootstrap 95% interval for the mean difference lies
   wholly below zero.

Within each five-seed arm, the test split is evaluated once at the final
fixed-token checkpoint and never drives early stopping. However, seed-0 pilot
test scores on the same enwik8 split were observed during architecture
calibration. The official seeds measure replication across initialization and
batch schedules, not fully untouched-dataset generalization. A result outside
this rule is inconclusive or favors the Transformer.

Two secondary measurements do not change the primary decision:

- recursive revision: second-round minus first-round held-out bpb, negative if
  the internal revision improved prediction;
- efficiency: steady training bytes/s, held-out inference bytes/s, peak RSS,
  parameter count, and scaling with context length.

Quality per parameter and runtime efficiency are separate claims. Sisyphus does
not earn an efficiency claim merely by winning bpb, and it does not earn a
quality claim from favorable asymptotic complexity.

## Corpus

- Dataset: `enwik8`, the first 100,000,000 bytes of the Hutter Prize Wikipedia
  corpus, downloaded from `https://mattmahoney.net/dc/enwik8.zip`.
- Uncompressed SHA-256:
  `2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8`.
- Vocabulary: all 256 byte values; bits/token therefore equals bits/byte.
- Immutable contiguous split: 90% train, 5% validation, 5% test.
- Training batches: precomputed random window starts. Each paired seed receives
  the identical start matrix, whose hash is recorded.
- Evaluation windows: deterministic, evenly spaced held-out starts.

`enwik8` is a recognized, difficult language-model/compression benchmark, but
this small CPU study is not directly comparable to large published enwik8
systems with millions or billions of parameters and far larger training
budgets.

## Models

At context 128:

- **Sisyphus:** one width-41 recursive block, two refinement rounds, seven
  dyadic causal scales, width-82 SwiGLU, tied byte embeddings, fixed sinusoidal
  positions at 0.02 amplitude, deep supervision, and a monotonic revision
  penalty; 29,811 physical/trainable parameters.
- **Transformer:** two width-32 Llama-style decoder blocks, four heads, RoPE,
  pre-RMSNorm, width-69 SwiGLU, bias-free projections, and tied byte
  embeddings; 29,792 physical/trainable parameters.

The 19-parameter difference is 0.064%. Sisyphus's quote/route/group/square cell
is shared across scale and refinement depth. Its physical count changes only by
one width-41 scale code when context doubles; the Transformer count is fixed.

## Fixed-token run

The official budget was frozen after the seed-0 development runs and before
any official seed was launched:

- context 128 bytes;
- 500 optimizer updates, batch size 4: exactly 256,000 training bytes per
  model, about 8.59 training bytes per parameter;
- AdamW, beta1 0.9, beta2 0.95, epsilon 1e-8, weight decay 0.1;
- global gradient clipping at 1.0;
- 40-update linear warmup followed by cosine decay from `3e-3` to `3e-4`;
- 32 fixed validation windows, evaluated every 100 updates;
- 64 fixed test windows, evaluated only at the final checkpoint;
- final fixed-token checkpoint selection, no early stopping;
- no continual trainer, checkpoint promotion, model growth, or DJ modulation.

The runner records the complete protocol and its SHA-256, dataset and schedule
hashes, per-step timings, environment, validation curve, final test result,
round-wise Sisyphus result, peak RSS, and checkpoints. Resume refuses a
protocol mismatch.

Official command (run from the Kaos repository root with a tinygrad 0.13.0
environment):

```bash
PYTHONPATH=. DEV=CPU python -m sisyphus benchmark \
  --corpus /path/to/enwik8 --require-enwik8 \
  --output-dir sisyphus/results/enwik8_v1 \
  --models sisyphus transformer --seeds 17 29 43 71 113 \
  --steps 500 --batch-size 4 --context-length 128 \
  --learning-rate 3e-3 --minimum-learning-rate 3e-4 \
  --warmup-steps 40 --eval-every 100 \
  --validation-windows 32 --test-windows 64 --eval-batch-size 4 \
  --no-resume

PYTHONPATH=. python -m sisyphus summarize \
  sisyphus/results/enwik8_v1 \
  --output sisyphus/results/enwik8_v1/summary.json
```

## Claim boundary

This study can establish a small-scale parameter/token-matched edge on one raw
byte corpus. It cannot establish state of the art, general reasoning ability,
safe autonomous self-improvement, superiority at scale, or lower wall-clock
cost. Sisyphus's continual improvement loop has a frozen external validation
gate because recursive self-training without real evidence is known to
collapse; the official architecture comparison disables that loop.

The subsequently frozen, no-pilot [text8 confirmation](CONFIRMATION.md) failed
0/5 and its paired interval wholly favors the Transformer. The enwik8 result is
therefore retained as a dataset-specific positive, not a general quality edge.

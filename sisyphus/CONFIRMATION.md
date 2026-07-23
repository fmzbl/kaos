# Frozen cross-corpus confirmation: text8

Status before execution: frozen; no pilot runs  
Frozen on: 2026-07-18

The enwik8 official-seed study passes its local paired rule, but seed-0 enwik8
test scores were observed during architecture calibration. This confirmation
tests the already-frozen Sisyphus implementation on a second standard corpus
that was not downloaded, evaluated, or used for any model decision beforehand.

## Immutable corpus

- Dataset: `text8`, the 100,000,000-byte cleaned Wikipedia corpus from
  `https://mattmahoney.net/dc/text8.zip`.
- Uncompressed SHA-256:
  `6e890197040d37d85beb962ae1f041ff1d9a9ca8d20c7d99c85027eebf51dca7`.
- Byte vocabulary: 256, without corpus-specific vocabulary fitting.
- Contiguous split: 90% train, 5% validation, 5% test.

## Frozen models and budget

No architecture or optimizer setting changes from enwik8 v1:

- Sisyphus: width 41, one shared block, two rounds, width-82 SwiGLU,
  29,811 parameters at context 128;
- Transformer: two width-32 blocks, four heads, width-69 SwiGLU,
  29,792 parameters;
- context 128, batch 4, 500 updates: 256,000 training bytes;
- AdamW, LR `3e-3` to `3e-4`, 40-step warmup, beta1 0.9, beta2 0.95,
  weight decay 0.1, gradient clip 1.0;
- 32 evenly spaced validation windows every 100 steps;
- 64 evenly spaced test windows once at the final fixed-token checkpoint;
- new paired seeds: `19, 31, 47, 73, 127`;
- no resume, growth, continual improvement, or tuning between pairs.

Decision rule is unchanged: all five pairs must finish, Sisyphus must win at
least four, and the paired-bootstrap 95% interval for Sisyphus minus Transformer
must lie wholly below zero. Recursive round two versus round one and runtime are
secondary.

## Frozen command

```bash
PYTHONPATH=. DEV=CPU python -m sisyphus benchmark \
  --corpus /path/to/text8 \
  --output-dir sisyphus/results/text8_v1 \
  --models sisyphus transformer --seeds 19 31 47 73 127 \
  --steps 500 --batch-size 4 --context-length 128 \
  --learning-rate 3e-3 --minimum-learning-rate 3e-4 \
  --warmup-steps 40 --eval-every 100 \
  --validation-windows 32 --test-windows 64 --eval-batch-size 4 \
  --no-resume
```

## Results

The confirmation rule **failed decisively**.

| Seed | Sisyphus bpb | Transformer bpb | Difference |
|---:|---:|---:|---:|
| 19 | 3.1545 | 3.0581 | +0.0963 |
| 31 | 3.1925 | 3.0302 | +0.1623 |
| 47 | 3.1460 | 3.0687 | +0.0773 |
| 73 | 3.1343 | 3.0938 | +0.0405 |
| 127 | 3.1191 | 3.0885 | +0.0305 |
| **Mean** | **3.1493** | **3.0679** | **+0.0814** |

The Transformer won all five pairs. The deterministic paired-bootstrap 95%
interval for Sisyphus minus Transformer is `[+0.0439, +0.1248]` bpb, wholly in
the control's favor. Sisyphus is 2.65% worse in mean bpb and the Transformer is
5.16× faster during context-128 training.

Internal recursion still works locally: round two improves round one on all
five seeds by 0.0363 bpb on average. That mechanistic improvement is not enough
to beat the control.

**Conclusion:** the enwik8 positive does not transfer to cleaned text8 under
the same architecture, optimizer, parameter count, and byte budget. Sisyphus
therefore has no demonstrated general short-context language-model quality
edge. The retained positive claim is dataset-specific enwik8 sample efficiency;
the independent long-context inference throughput/memory edge is unaffected by
this quality failure. Complete records are in
[`results/text8_v1/summary.json`](results/text8_v1/summary.json).

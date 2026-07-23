# Sisyphus in the Rebis–Kaos stack

Sisyphus is the root-level language-model component under
[`sisyphus`](../sisyphus). It is a reimagining of Thoth's causal
counter-recursive decoder around Rebis's small semantic basis and Kaos's
bounded, evidence-gated improvement policy.

```text
Rebis semantics       Sisyphus neural mixer       Kaos policy
quote / route    ->   retain / causal shift   ->  immutable intent
group / square   ->   compose / mediate       ->  external gate
macro recursion  ->   shared refinement cell  ->  Fibonacci retries
evolve judge     ->   deep revision loss      ->  promote or rollback
```

The architecture, trainer, evidence, and command surface are now first-class
parts of the repository. The tinygrad execution stack remains operationally
separate from the Rust application so architecture experiments cannot
destabilize the terminal runtime. Kaos can consume a future scaled checkpoint
through an Ollama/OpenAI serving seam; the present 30k-parameter checkpoint is
not represented as a production provider.

## Measured results

The primary benchmark is the exact 100MB enwik8 corpus, not a project-private
toy set. Five fixed seeds compare Sisyphus with a modern Transformer at 29,811
versus 29,792 parameters, identical raw-byte batches, and exactly 256,000
training bytes per arm.

| Metric | Sisyphus | Transformer |
|---|---:|---:|
| Mean held-out bits/byte | **3.7491** | 3.8013 |
| Paired wins | **4/5** | 1/5 |
| Steady context-128 training bytes/s | 1,025 | **5,213** |
| Context-4,096 inference bytes/s | **30,920** | 6,140 |
| Context-4,096 peak RSS | **175 MiB** | 1,049 MiB |

Mean paired difference is −0.0522 bpb; deterministic paired-bootstrap 95%
interval is `[−0.0971, −0.0061]`. The predeclared narrow quality rule passes.
Round two improves round one in all five seeds by 0.0645 bpb on average.

This was not an untouched-corpus result: seed-0 enwik8 test scores had been
observed during architecture calibration. Every setting was therefore frozen
before a no-pilot confirmation on the separate 100MB text8 corpus:

| Confirmation metric | Sisyphus | Transformer |
|---|---:|---:|
| Mean held-out bits/byte | 3.1493 | **3.0679** |
| Paired wins | 0/5 | **5/5** |
| Paired difference | +0.0814 | |
| Paired-bootstrap 95% CI | `[+0.0439, +0.1248]` | |

The confirmation decisively fails the quality rule. Sisyphus has no
demonstrated general short-context language-quality edge; the enwik8 result is
a retained dataset-specific positive. Recursive revision does transfer as a
mechanism—round two also improves round one in all five text8 seeds—but does
not make the model better than the control.

The efficiency statement has two regimes:

- at context 128, Sisyphus is more sample-efficient but the Transformer trains
  5.09× faster;
- isolated CPU inference crosses over among measured lengths at context 2,048;
  at 4,096 Sisyphus is 5.04× faster with 83.3% lower peak RSS.

See the [architecture](../sisyphus/ARCHITECTURE.md),
[paper](../sisyphus/PAPER.md), [frozen protocol](../sisyphus/PROTOCOL.md),
[enwik8 record](../sisyphus/results/enwik8_v1/summary.json),
[untouched confirmation](../sisyphus/CONFIRMATION.md), and
[scaling record](../sisyphus/results/scaling_v1/summary.json).

## What “self-improving” means here

Sisyphus improves at two bounded levels:

1. **Activation revision.** The same cell revisits the sequence twice; later
   predictions receive deep supervision and a penalty for worsening the prior
   pass.
2. **Weight promotion.** [`improve.py`](../sisyphus/improve.py) trains a
   candidate descended from the champion on real training bytes. A frozen
   validation gate either atomically promotes it or leaves the champion hash
   unchanged. Fibonacci budgets and cold/hot optimizer polarity diversify
   retries; every decision is ledgered.

It does not recursively ingest its own generated text, change its objective,
move the held-out boundary, or approve itself with an LLM judge. Those are
collapse and reward-hacking mechanisms, not improvement.

## Reproduce and extend

Install the small Python environment and run the exact commands in the
[Sisyphus README](../sisyphus/README.md). The next defensible study is
new seeds on at least two more predeclared real corpora, several parameter
scales, and both fixed-token and fixed-wall-clock curves. Do not tune further
on either retained `enwik8_v1` or `text8_v1` test results.

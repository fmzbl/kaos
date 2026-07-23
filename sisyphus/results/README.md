# Retained Sisyphus measurements

- `enwik8_v1` contains all ten official fixed-token records, final checkpoints,
  checkpoint hashes, and the paired summary. Per-run JSON includes the full
  protocol, implementation hashes, corpus/schedule hashes, environment,
  validation curve, timings, final test metrics, and Sisyphus round metrics.
- `text8_v1` contains the ten records and checkpoints for the frozen no-pilot
  cross-corpus confirmation. The Transformer won all five pairs and the paired
  interval wholly favors it; this retained failure rules out a claimed general
  short-context quality edge.
- `scaling_v1` contains isolated-process batch-one CPU inference measurements
  at contexts 32 through 4,096 and their paired crossover summary.

Seed-0 development runs live outside the repository under `/tmp` and are not
part of the official evidence. Neither 100MB corpus is redistributed; sources
and exact uncompressed hashes are in [`../PROTOCOL.md`](../PROTOCOL.md) and
[`../CONFIRMATION.md`](../CONFIRMATION.md).

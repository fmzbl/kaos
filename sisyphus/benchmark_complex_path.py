"""The H1 smallest disproving experiment: four matched-parameter arms.

Runs the complex Rebis path machine, its matched real-valued ablation, its
non-recursive ablation, and its phase-destroyed ablation on an identical,
byte-identical training schedule per seed, on the deterministic synthetic
corpus in ``synthetic_data.py``.  This is a pilot discriminating study, not a
replacement for the frozen ``sisyphus-byte-v1`` enwik8/text8 protocol: it
exists to falsify or fail-to-falsify H1 as cheaply as possible before any
larger, more expensive study is justified.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import time
from pathlib import Path
from typing import Any

from tinygrad import Device, Tensor, TinyJit
from tinygrad.nn.optim import AdamW

from .benchmark_lm import _evaluate
from .compiler import ensure_compiler
from .complex_path import RebisPathLM, build_path_model
from .data import BatchSchedule, ByteCorpus
from .synthetic_data import sha256_of, write_synthetic_corpus

PROTOCOL_VERSION = "rebis-path-pilot-v1"


def _implementation_hashes() -> dict[str, str]:
    root = Path(__file__).resolve().parent
    return {
        name: hashlib.sha256((root / name).read_bytes()).hexdigest()
        for name in ("benchmark_complex_path.py", "complex_path.py", "synthetic_data.py")
    }


def _write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def run_one(args: argparse.Namespace, arm: str, seed: int, corpus: ByteCorpus) -> dict[str, Any]:
    ensure_compiler()
    data_seed = seed + args.data_seed_offset
    schedule = BatchSchedule(
        corpus,
        steps=args.steps,
        batch_size=args.batch_size,
        context_length=args.context_length,
        seed=data_seed,
    )
    Tensor.manual_seed(seed)
    model, spec, parameters = build_path_model(
        arm,
        context_length=args.context_length,
        vocab_size=256,
        width=args.width,
        rounds=args.rounds,
        ffn_hidden=args.ffn_hidden,
    )
    trainable = model.optimizer_parameters()
    optimizer = AdamW(
        trainable,
        lr=args.learning_rate,
        b1=0.9,
        b2=0.95,
        eps=1e-8,
        weight_decay=args.weight_decay,
    )

    @TinyJit
    def train_step(inputs: Tensor, targets: Tensor) -> Tensor:
        with Tensor.train():
            optimizer.zero_grad()
            objective = model.loss(inputs, targets)
            objective.backward()
            gradients = [value.grad for value in trainable if value.grad is not None]
            norm = sum(
                (gradient.square().sum() for gradient in gradients), Tensor.zeros(())
            ).sqrt()
            scale = (args.gradient_clip / (norm + 1e-6)).minimum(1.0)
            for value in trainable:
                if value.grad is not None:
                    value.grad = value.grad * scale
            optimizer.step()
            return objective.realize()

    validation = corpus.evaluation_batches(
        "validation",
        context_length=args.context_length,
        windows=args.validation_windows,
        batch_size=args.eval_batch_size,
    )
    test = corpus.evaluation_batches(
        "test",
        context_length=args.context_length,
        windows=args.test_windows,
        batch_size=args.eval_batch_size,
    )

    started = time.perf_counter()
    train_losses: list[float] = []
    for step in range(args.steps):
        x_array, y_array = schedule.batch(corpus, step, args.context_length)
        train_losses.append(float(train_step(Tensor(x_array), Tensor(y_array)).item()))
    elapsed = time.perf_counter() - started

    final_validation = _evaluate(model, validation)
    final_test = _evaluate(model, test)
    diagnostics = model.diagnostics(Tensor(test[0][0]))

    run_id = f"{arm}.seed_{seed}.steps_{args.steps}.ctx_{args.context_length}"
    result = {
        "status": "complete",
        "run_id": run_id,
        "protocol": {
            "version": PROTOCOL_VERSION,
            "implementation_sha256": _implementation_hashes(),
            "model": spec.to_dict(),
            "parameters": parameters,
            "seed": seed,
            "data_seed": data_seed,
            "corpus_sha256": corpus.metadata.sha256,
            "corpus_bytes": corpus.metadata.bytes,
            "schedule_sha256": schedule.sha256,
            "steps": args.steps,
            "batch_size": args.batch_size,
            "context_length": args.context_length,
        },
        "environment": {"platform": platform.platform(), "device": Device.DEFAULT},
        "train_first_loss": train_losses[0],
        "train_final_loss": train_losses[-1],
        "training_elapsed_seconds": elapsed,
        "final_validation": final_validation,
        "final_test": final_test,
        "path_diagnostics_last_batch": diagnostics,
    }
    result_path = args.output_dir / f"{run_id}.json"
    _write_json(result_path, result)
    print(
        f"{run_id} params={parameters:,} test={final_test['bits_per_byte']:.4f} bpb "
        f"elapsed={elapsed:.1f}s",
        flush=True,
    )
    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--corpus-path", type=Path, required=True)
    parser.add_argument("--corpus-bytes", type=int, default=200_000)
    parser.add_argument("--corpus-seed", type=int, default=20260720)
    parser.add_argument(
        "--arms", nargs="+", choices=RebisPathLM.ARMS, default=list(RebisPathLM.ARMS)
    )
    parser.add_argument("--seeds", nargs="+", type=int, default=(17, 29, 43))
    parser.add_argument("--steps", type=int, default=150)
    parser.add_argument("--batch-size", type=int, default=4)
    parser.add_argument("--context-length", type=int, default=128)
    parser.add_argument("--width", type=int, default=20)
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--ffn-hidden", type=int, default=80)
    parser.add_argument("--learning-rate", type=float, default=3e-3)
    parser.add_argument("--weight-decay", type=float, default=0.1)
    parser.add_argument("--gradient-clip", type=float, default=1.0)
    parser.add_argument("--validation-windows", type=int, default=16)
    parser.add_argument("--test-windows", type=int, default=32)
    parser.add_argument("--eval-batch-size", type=int, default=4)
    parser.add_argument("--data-seed-offset", type=int, default=10_000)
    args = parser.parse_args()
    if args.context_length < 2 or args.context_length & (args.context_length - 1):
        parser.error("context length must be a power of two >= 2")
    args.output_dir = args.output_dir.expanduser().resolve()
    return args


def main() -> None:
    args = parse_args()
    corpus_path = write_synthetic_corpus(args.corpus_path, args.corpus_bytes, args.corpus_seed)
    print(
        f"synthetic corpus: {corpus_path} ({args.corpus_bytes} bytes, "
        f"seed={args.corpus_seed}, sha256={sha256_of(corpus_path)})",
        flush=True,
    )
    corpus = ByteCorpus(corpus_path)
    for seed in args.seeds:
        for arm in args.arms:
            run_one(args, arm, seed, corpus)


if __name__ == "__main__":
    main()

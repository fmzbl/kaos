"""Promotion-gated continual improvement for Sisyphus.

Each candidate begins from the current champion, trains only on the corpus's
training split, and may replace the champion only after improving a frozen set
of validation windows by a declared margin.  Rejection is an exact rollback:
the champion checkpoint is never mutated in place.  This is intentionally not
closed-loop self-training on generated text; that failure mode amplifies model
errors and is not evidence-grounded improvement.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import time
from pathlib import Path
from typing import Any

import numpy as np
from tinygrad import Tensor, TinyJit
from tinygrad.nn.optim import AdamW
from tinygrad.nn.state import get_state_dict, load_state_dict, safe_load, safe_save

from .benchmark_lm import _evaluate
from .compiler import ensure_compiler
from .data import BatchSchedule, ByteCorpus
from .models import build_model


def fib(index: int) -> int:
    a, b = 1, 1
    for _ in range(index):
        a, b = b, a + b
    return a


def _write_json(path: Path, value: dict[str, Any]) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def _save_model(path: Path, model, metadata: dict[str, str]) -> None:
    state = {f"model.{key}": value for key, value in get_state_dict(model).items()}
    temporary = path.with_name(path.stem + ".tmp.safetensors")
    safe_save(state, str(temporary), metadata=metadata)
    os.replace(temporary, path)


def _load_model(path: Path, model) -> None:
    state = safe_load(str(path))
    load_state_dict(
        model,
        {
            key.removeprefix("model."): value
            for key, value in state.items()
            if key.startswith("model.")
        },
    )


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _train_candidate(model, corpus: ByteCorpus, args, attempt: int, steps: int) -> dict:
    # Consecutive failed attempts alternate a convergent and exploratory LR,
    # Kaos's solar/lunar restart polarity expressed in optimizer space.
    polarity = "solar" if attempt % 2 == 0 else "lunar"
    multiplier = 0.75 if polarity == "solar" else 1.25
    learning_rate = args.learning_rate * multiplier
    parameters = model.optimizer_parameters()
    optimizer = AdamW(
        parameters,
        lr=learning_rate,
        b1=0.9,
        b2=0.95,
        eps=1e-8,
        weight_decay=args.weight_decay,
    )
    schedule = BatchSchedule(
        corpus,
        steps=steps,
        batch_size=args.batch_size,
        context_length=args.context_length,
        seed=args.seed + 10_000 + attempt,
    )

    @TinyJit
    def train_step(inputs: Tensor, targets: Tensor) -> Tensor:
        with Tensor.train():
            optimizer.zero_grad()
            loss = model.loss(inputs, targets)
            loss.backward()
            gradients = [value.grad for value in parameters if value.grad is not None]
            norm = sum(
                (gradient.square().sum() for gradient in gradients), Tensor.zeros(())
            ).sqrt()
            scale = (args.gradient_clip / (norm + 1e-6)).minimum(1.0)
            for value in parameters:
                if value.grad is not None:
                    value.grad = value.grad * scale
            optimizer.step()
            return loss.realize()

    losses = []
    started = time.perf_counter()
    for step in range(steps):
        x_array, y_array = schedule.batch(corpus, step, args.context_length)
        losses.append(float(train_step(Tensor(x_array), Tensor(y_array)).item()))
    return {
        "polarity": polarity,
        "steps": steps,
        "trained_bytes": steps * args.batch_size * args.context_length,
        "learning_rate": learning_rate,
        "schedule_sha256": schedule.sha256,
        "first_objective": losses[0],
        "final_objective": losses[-1],
        "elapsed_seconds": time.perf_counter() - started,
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    ensure_compiler()
    args.state_dir.mkdir(parents=True, exist_ok=True)
    candidates = args.state_dir / "candidates"
    candidates.mkdir(exist_ok=True)
    ledger_path = args.state_dir / "ledger.json"
    champion_path = args.state_dir / "champion.safetensors"
    corpus = ByteCorpus(args.corpus)
    gate_batches = corpus.evaluation_batches(
        "validation",
        context_length=args.context_length,
        windows=args.gate_windows,
        batch_size=args.eval_batch_size,
    )
    protocol = {
        "model": "sisyphus",
        "corpus_sha256": corpus.metadata.sha256,
        "context_length": args.context_length,
        "gate": "fixed validation bpb; lower by min_delta promotes",
        "gate_windows": args.gate_windows,
        "eval_batch_size": args.eval_batch_size,
        "min_delta_bpb": args.min_delta_bpb,
        "test_split_used": False,
        "self_generated_training_data": False,
    }
    ledger: dict[str, Any] = (
        json.loads(ledger_path.read_text())
        if ledger_path.exists()
        else {"protocol": protocol, "attempts": []}
    )
    if ledger["protocol"] != protocol:
        raise ValueError("state directory belongs to a different improvement protocol")

    if not champion_path.exists():
        Tensor.manual_seed(args.seed)
        initial, _, _ = build_model("sisyphus", context_length=args.context_length)
        _save_model(champion_path, initial, {"status": "initial"})
        ledger["initial"] = {
            "validation": _evaluate(initial, gate_batches),
            "checkpoint_sha256": _sha256(champion_path),
        }
        _write_json(ledger_path, ledger)

    for _ in range(args.attempts):
        attempt = len(ledger["attempts"])
        Tensor.manual_seed(args.seed + attempt + 1)
        champion, _, _ = build_model("sisyphus", context_length=args.context_length)
        _load_model(champion_path, champion)
        before = _evaluate(champion, gate_batches)

        candidate, _, _ = build_model("sisyphus", context_length=args.context_length)
        _load_model(champion_path, candidate)
        steps = args.step_unit * fib(attempt + 4)  # 5, 8, 13, 21, ... units
        training = _train_candidate(candidate, corpus, args, attempt, steps)
        after = _evaluate(candidate, gate_batches)
        delta = after["bits_per_byte"] - before["bits_per_byte"]
        promoted = delta <= -args.min_delta_bpb
        candidate_path = candidates / f"attempt_{attempt:04d}.safetensors"
        _save_model(
            candidate_path,
            candidate,
            {"status": "promoted" if promoted else "rejected", "attempt": str(attempt)},
        )
        if promoted:
            # Write a new complete file, then atomically replace the champion.
            replacement = args.state_dir / "champion.next.safetensors"
            _save_model(replacement, candidate, {"status": "champion", "attempt": str(attempt)})
            os.replace(replacement, champion_path)
        record = {
            "attempt": attempt,
            "champion_before_bpb": before["bits_per_byte"],
            "candidate_bpb": after["bits_per_byte"],
            "delta_bpb": delta,
            "promoted": promoted,
            "training": training,
            "candidate_checkpoint": str(candidate_path),
            "candidate_sha256": _sha256(candidate_path),
            "champion_sha256_after": _sha256(champion_path),
        }
        ledger["attempts"].append(record)
        _write_json(ledger_path, ledger)
        print(json.dumps(record, sort_keys=True), flush=True)
    return ledger


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--attempts", type=int, default=1)
    parser.add_argument("--context-length", type=int, default=128)
    parser.add_argument("--batch-size", type=int, default=4)
    parser.add_argument("--step-unit", type=int, default=10)
    parser.add_argument("--learning-rate", type=float, default=1e-3)
    parser.add_argument("--weight-decay", type=float, default=0.1)
    parser.add_argument("--gradient-clip", type=float, default=1.0)
    parser.add_argument("--gate-windows", type=int, default=64)
    parser.add_argument("--eval-batch-size", type=int, default=4)
    parser.add_argument("--min-delta-bpb", type=float, default=0.001)
    parser.add_argument("--seed", type=int, default=20260718)
    args = parser.parse_args()
    if args.context_length < 2 or args.context_length & (args.context_length - 1):
        parser.error("context length must be a power of two >= 2")
    if args.attempts < 1 or args.step_unit < 1 or args.min_delta_bpb < 0:
        parser.error("attempts/step-unit must be positive and min-delta nonnegative")
    args.corpus = args.corpus.expanduser().resolve()
    args.state_dir = args.state_dir.expanduser().resolve()
    return args


def main() -> None:
    run(parse_args())


if __name__ == "__main__":
    main()

"""Matched Sisyphus-versus-Transformer raw-byte language-model benchmark."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import resource
import sys
import time
from pathlib import Path
from typing import Any

from tinygrad import Device, Tensor, TinyJit
from tinygrad.nn.optim import AdamW
from tinygrad.nn.state import get_state_dict, load_state_dict, safe_load, safe_save

from .compiler import ensure_compiler
from .data import BatchSchedule, ByteCorpus
from .models import SisyphusLM, build_model

PROTOCOL_VERSION = "sisyphus-byte-v1"
OFFICIAL_ENWIK8_SHA256 = "2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8"


def _implementation_hashes() -> dict[str, str]:
    root = Path(__file__).resolve().parent
    return {
        name: hashlib.sha256((root / name).read_bytes()).hexdigest()
        for name in ("benchmark_lm.py", "data.py", "layers.py", "models.py")
    }


def _write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def _learning_rate(
    step: int, *, total: int, maximum: float, minimum: float, warmup: int
) -> float:
    if warmup and step <= warmup:
        return maximum * step / warmup
    progress = min(1.0, max(0.0, (step - warmup) / max(1, total - warmup)))
    cosine = 0.5 * (1.0 + math.cos(math.pi * progress))
    return minimum + (maximum - minimum) * cosine


def _evaluate(model, batches) -> dict[str, Any]:
    language_total = 0.0
    round_totals: list[float] | None = None
    tokens = 0
    started = time.perf_counter()
    for x_array, y_array in batches:
        inputs, targets = Tensor(x_array), Tensor(y_array)
        if isinstance(model, SisyphusLM):
            values = model.logits_by_round(inputs)
            round_losses = [
                float(
                    value.reshape(-1, model.vocab_size)
                    .sparse_categorical_crossentropy(targets.reshape(-1))
                    .item()
                )
                for value in values
            ]
            if round_totals is None:
                round_totals = [0.0] * len(round_losses)
            for index, loss in enumerate(round_losses):
                round_totals[index] += loss * y_array.size
            language = round_losses[-1]
        else:
            language = float(model.language_loss(inputs, targets).item())
        language_total += language * y_array.size
        tokens += int(y_array.size)
    elapsed = time.perf_counter() - started
    nats = language_total / tokens
    result: dict[str, Any] = {
        "tokens": tokens,
        "nats_per_byte": nats,
        "bits_per_byte": nats / math.log(2.0),
        "perplexity": math.exp(min(nats, 700.0)),
        "elapsed_seconds": elapsed,
        "tokens_per_second": tokens / max(elapsed, 1e-12),
    }
    if round_totals is not None:
        result["round_bits_per_byte"] = [
            total / tokens / math.log(2.0) for total in round_totals
        ]
    return result


def _checkpoint(path: Path, model, optimizer, step: int) -> None:
    state = {
        **{f"model.{key}": value for key, value in get_state_dict(model).items()},
        **{f"optimizer.{key}": value for key, value in get_state_dict(optimizer).items()},
        "benchmark.step": Tensor([step]),
    }
    temporary = path.with_name(path.stem + ".tmp.safetensors")
    safe_save(state, str(temporary), metadata={"protocol": PROTOCOL_VERSION})
    os.replace(temporary, path)


def _restore(path: Path, model, optimizer) -> int:
    state = safe_load(str(path))
    load_state_dict(
        model,
        {
            key.removeprefix("model."): value
            for key, value in state.items()
            if key.startswith("model.")
        },
    )
    load_state_dict(
        optimizer,
        {
            key.removeprefix("optimizer."): value
            for key, value in state.items()
            if key.startswith("optimizer.")
        },
    )
    return int(state["benchmark.step"].item())


def run_one(args: argparse.Namespace, model_name: str, seed: int) -> dict[str, Any]:
    ensure_compiler()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    corpus = ByteCorpus(args.corpus)
    if args.require_enwik8 and (
        corpus.metadata.bytes != 100_000_000
        or (
            corpus.metadata.sha256 != OFFICIAL_ENWIK8_SHA256
        )
    ):
        raise ValueError("--require-enwik8 needs the exact 100,000,000-byte corpus")
    data_seed = seed + args.data_seed_offset
    schedule = BatchSchedule(
        corpus,
        steps=args.steps,
        batch_size=args.batch_size,
        context_length=args.context_length,
        seed=data_seed,
    )
    Tensor.manual_seed(seed)
    model, spec, parameters = build_model(
        model_name, context_length=args.context_length, vocab_size=256
    )
    trainable = model.optimizer_parameters()
    optimizer = AdamW(
        trainable,
        lr=args.learning_rate,
        b1=args.beta1,
        b2=args.beta2,
        eps=args.epsilon,
        weight_decay=args.weight_decay,
    )
    run_id = f"{model_name}.seed_{seed}.steps_{args.steps}.ctx_{args.context_length}"
    result_path = args.output_dir / f"{run_id}.json"
    checkpoint_path = args.output_dir / f"{run_id}.safetensors"
    protocol = {
        "version": PROTOCOL_VERSION,
        "implementation_sha256": _implementation_hashes(),
        "model": spec.to_dict(),
        "parameters": parameters,
        "trainable_parameters": sum(value.numel() for value in trainable),
        "seed": seed,
        "data_seed": data_seed,
        "dataset": corpus.metadata.to_dict(),
        "schedule_sha256": schedule.sha256,
        "steps": args.steps,
        "batch_size": args.batch_size,
        "context_length": args.context_length,
        "trained_tokens": args.steps * args.batch_size * args.context_length,
        "optimizer": {
            "name": "AdamW",
            "learning_rate": args.learning_rate,
            "minimum_learning_rate": args.minimum_learning_rate,
            "beta1": args.beta1,
            "beta2": args.beta2,
            "epsilon": args.epsilon,
            "weight_decay": args.weight_decay,
            "gradient_clip": args.gradient_clip,
            "warmup_steps": args.warmup_steps,
            "schedule": "linear-warmup-cosine",
        },
        "evaluation": {
            "validation_windows": args.validation_windows,
            "test_windows": args.test_windows,
            "selection": "final fixed-token checkpoint",
        },
    }
    protocol_hash = hashlib.sha256(json.dumps(protocol, sort_keys=True).encode()).hexdigest()
    history: list[dict[str, Any]] = []
    durations: list[float] = []
    start_step = 0
    elapsed_before = 0.0
    if result_path.exists() and args.resume:
        existing = json.loads(result_path.read_text())
        if existing.get("protocol_sha256") != protocol_hash:
            raise ValueError(f"refusing protocol-mismatched resume for {run_id}")
        if existing.get("status") == "complete":
            print(f"skip complete {run_id}", flush=True)
            return existing
        history = existing.get("history", [])
        durations = existing.get("step_durations_seconds", [])
        elapsed_before = float(existing.get("training_elapsed_seconds", 0.0))
        if checkpoint_path.exists():
            start_step = _restore(checkpoint_path, model, optimizer)

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

    wall_started = time.perf_counter()
    latest_validation = history[-1]["validation"] if history else None
    if start_step == 0:
        latest_validation = _evaluate(model, validation)
        history.append({"step": 0, "validation": latest_validation})
        print(
            f"{run_id} params={parameters:,} initial={latest_validation['bits_per_byte']:.4f} bpb",
            flush=True,
        )
    for zero_step in range(start_step, args.steps):
        step = zero_step + 1
        learning_rate = _learning_rate(
            step,
            total=args.steps,
            maximum=args.learning_rate,
            minimum=args.minimum_learning_rate,
            warmup=args.warmup_steps,
        )
        optimizer.lr.assign(Tensor([learning_rate], device=optimizer.lr.device)).realize()
        x_array, y_array = schedule.batch(corpus, zero_step, args.context_length)
        started = time.perf_counter()
        objective = float(train_step(Tensor(x_array), Tensor(y_array)).item())
        durations.append(time.perf_counter() - started)
        if step % args.eval_every == 0 or step == args.steps:
            latest_validation = _evaluate(model, validation)
            history.append(
                {
                    "step": step,
                    "train_objective": objective,
                    "learning_rate": learning_rate,
                    "validation": latest_validation,
                }
            )
            partial = {
                "status": "running",
                "run_id": run_id,
                "protocol": protocol,
                "protocol_sha256": protocol_hash,
                "completed_steps": step,
                "training_elapsed_seconds": elapsed_before
                + time.perf_counter()
                - wall_started,
                "history": history,
                "step_durations_seconds": durations,
            }
            _checkpoint(checkpoint_path, model, optimizer, step)
            _write_json(result_path, partial)
            print(
                f"{run_id} step={step}/{args.steps} objective={objective:.4f} "
                f"val={latest_validation['bits_per_byte']:.4f} bpb "
                f"step_s={durations[-1]:.3f}",
                flush=True,
            )

    final_test = _evaluate(model, test)
    warmup_count = min(3, len(durations))
    steady_seconds = sum(durations[warmup_count:])
    steady_tokens = max(0, len(durations) - warmup_count) * args.batch_size * args.context_length
    result = {
        "status": "complete",
        "run_id": run_id,
        "protocol": protocol,
        "protocol_sha256": protocol_hash,
        "environment": {
            "platform": platform.platform(),
            "python": sys.version,
            "device": Device.DEFAULT,
        },
        "completed_steps": args.steps,
        "training_elapsed_seconds": elapsed_before + time.perf_counter() - wall_started,
        "compile_warmup_seconds": sum(durations[:warmup_count]),
        "steady_training_tokens_per_second": (
            steady_tokens / steady_seconds if steady_seconds else None
        ),
        "peak_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
        "history": history,
        "step_durations_seconds": durations,
        "final_validation": latest_validation,
        "final_test": final_test,
    }
    _checkpoint(checkpoint_path, model, optimizer, args.steps)
    _write_json(result_path, result)
    print(
        f"complete {run_id}: test={final_test['bits_per_byte']:.4f} bpb "
        f"steady={result['steady_training_tokens_per_second'] or 0:.1f} tok/s",
        flush=True,
    )
    return result


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--models", nargs="+", choices=("sisyphus", "transformer"), default=("sisyphus", "transformer"))
    parser.add_argument("--seeds", nargs="+", type=int, default=(17, 29, 43, 71, 113))
    parser.add_argument("--steps", type=int, default=500)
    parser.add_argument("--batch-size", type=int, default=4)
    parser.add_argument("--context-length", type=int, default=128)
    parser.add_argument("--learning-rate", type=float, default=3e-3)
    parser.add_argument("--minimum-learning-rate", type=float, default=3e-4)
    parser.add_argument("--warmup-steps", type=int, default=40)
    parser.add_argument("--weight-decay", type=float, default=0.1)
    parser.add_argument("--beta1", type=float, default=0.9)
    parser.add_argument("--beta2", type=float, default=0.95)
    parser.add_argument("--epsilon", type=float, default=1e-8)
    parser.add_argument("--gradient-clip", type=float, default=1.0)
    parser.add_argument("--eval-every", type=int, default=100)
    parser.add_argument("--validation-windows", type=int, default=32)
    parser.add_argument("--test-windows", type=int, default=64)
    parser.add_argument("--eval-batch-size", type=int, default=4)
    parser.add_argument("--data-seed-offset", type=int, default=10_000)
    parser.add_argument("--resume", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--require-enwik8", action="store_true")
    args = parser.parse_args()
    if args.context_length < 2 or args.context_length & (args.context_length - 1):
        parser.error("context length must be a power of two >= 2")
    if args.minimum_learning_rate > args.learning_rate:
        parser.error("minimum learning rate cannot exceed learning rate")
    args.corpus = args.corpus.expanduser().resolve()
    args.output_dir = args.output_dir.expanduser().resolve()
    return args


def main() -> None:
    args = parse_args()
    for seed in args.seeds:
        for model in args.models:
            run_one(args, model, seed)


if __name__ == "__main__":
    main()

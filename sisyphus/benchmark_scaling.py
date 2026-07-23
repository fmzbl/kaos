"""Measure Sisyphus and Transformer inference scaling at fixed parameters."""

from __future__ import annotations

import argparse
import json
import math
import platform
import resource
import time
from pathlib import Path

import numpy as np
from tinygrad import Device, Tensor, TinyJit

from .compiler import ensure_compiler
from .models import build_model


def run(model_name: str, context: int, repeats: int, output: Path | None) -> dict:
    ensure_compiler()
    Tensor.manual_seed(20260718)
    model, spec, parameters = build_model(model_name, context_length=context)
    values = np.arange(context, dtype=np.int32)[None, :] % 256
    tokens = Tensor(values)

    @TinyJit
    def forward(value: Tensor) -> Tensor:
        return model(value).realize()

    # The first realization compiles; the second populates the JIT capture.
    forward(tokens)
    forward(tokens)
    durations = []
    for _ in range(repeats):
        started = time.perf_counter()
        forward(tokens)
        durations.append(time.perf_counter() - started)
    median = float(np.median(durations))
    result = {
        "model": model_name,
        "context_length": context,
        "parameters": parameters,
        "repeats": repeats,
        "median_seconds": median,
        "tokens_per_second": context / median,
        "peak_rss_kib": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
        "environment": {"device": Device.DEFAULT, "platform": platform.platform()},
        "structural_work_units": (
            spec.layers * (spec.rounds or 1) * int(math.log2(context)) * context
            if model_name == "sisyphus"
            else spec.layers * (spec.heads or 1) * context * context
        ),
        "structural_work_definition": (
            "layers * rounds * log2(context) * positions"
            if model_name == "sisyphus"
            else "layers * heads * context^2 attention scores"
        ),
    }
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if output:
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(rendered)
    print(rendered, end="")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", choices=("sisyphus", "transformer"), required=True)
    parser.add_argument("--context", type=int, required=True)
    parser.add_argument("--repeats", type=int, default=10)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.context < 2 or args.context & (args.context - 1):
        parser.error("context must be a power of two >= 2")
    run(args.model, args.context, args.repeats, args.output)


if __name__ == "__main__":
    main()


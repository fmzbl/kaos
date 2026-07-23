"""Pair isolated scaling records and report the measured crossover."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def summarize(directory: Path) -> dict:
    records = [
        json.loads(path.read_text())
        for path in directory.glob("*.json")
        if path.name != "summary.json"
    ]
    indexed = {(item["model"], item["context_length"]): item for item in records}
    contexts = sorted(
        set(context for model, context in indexed if model == "sisyphus")
        & set(context for model, context in indexed if model == "transformer")
    )
    pairs = []
    for context in contexts:
        sisyphus = indexed[("sisyphus", context)]
        transformer = indexed[("transformer", context)]
        pairs.append(
            {
                "context_length": context,
                "sisyphus_parameters": sisyphus["parameters"],
                "transformer_parameters": transformer["parameters"],
                "sisyphus_tokens_per_second": sisyphus["tokens_per_second"],
                "transformer_tokens_per_second": transformer["tokens_per_second"],
                "sisyphus_speedup": sisyphus["tokens_per_second"]
                / transformer["tokens_per_second"],
                "sisyphus_peak_rss_kib": sisyphus["peak_rss_kib"],
                "transformer_peak_rss_kib": transformer["peak_rss_kib"],
                "sisyphus_peak_rss_reduction_percent": (
                    1.0 - sisyphus["peak_rss_kib"] / transformer["peak_rss_kib"]
                )
                * 100.0,
            }
        )
    faster = [pair["context_length"] for pair in pairs if pair["sisyphus_speedup"] > 1]
    return {
        "pairs": pairs,
        "measured_inference_crossover_context": min(faster) if faster else None,
        "claim": (
            "Sisyphus has a measured CPU inference throughput edge at and above the "
            "reported crossover among tested contexts only."
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = summarize(args.directory)
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered)
    print(rendered, end="")


if __name__ == "__main__":
    main()


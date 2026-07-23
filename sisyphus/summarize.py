"""Summarize paired benchmark records with a deterministic bootstrap interval."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np


def _stats(values: list[float]) -> dict[str, float | int]:
    array = np.asarray(values, dtype=np.float64)
    return {
        "n": len(values),
        "mean": float(array.mean()),
        "sample_std": float(array.std(ddof=1)) if len(values) > 1 else 0.0,
        "standard_error": float(array.std(ddof=1) / np.sqrt(len(values)))
        if len(values) > 1
        else 0.0,
    }


def summarize(directory: Path) -> dict:
    runs = [
        json.loads(path.read_text())
        for path in sorted(directory.glob("*.json"))
        if path.name != "summary.json"
    ]
    runs = [run for run in runs if run.get("status") == "complete"]
    indexed = {
        (run["protocol"]["model"]["name"], run["protocol"]["seed"]): run
        for run in runs
    }
    seeds = sorted(
        set(seed for model, seed in indexed if model == "sisyphus")
        & set(seed for model, seed in indexed if model == "transformer")
    )
    differences = [
        indexed[("sisyphus", seed)]["final_test"]["bits_per_byte"]
        - indexed[("transformer", seed)]["final_test"]["bits_per_byte"]
        for seed in seeds
    ]
    random = np.random.default_rng(20260718)
    if differences:
        array = np.asarray(differences)
        samples = random.choice(array, size=(100_000, len(array)), replace=True).mean(axis=1)
        interval = [float(value) for value in np.quantile(samples, (0.025, 0.975))]
    else:
        interval = [None, None]
    models = {}
    for name in ("sisyphus", "transformer"):
        selected = [indexed[(name, seed)] for seed in seeds]
        quality = [run["final_test"]["bits_per_byte"] for run in selected]
        throughput = [run["steady_training_tokens_per_second"] for run in selected]
        inference = [run["final_test"]["tokens_per_second"] for run in selected]
        peak_rss = [float(run["peak_rss_kib"]) for run in selected]
        elapsed = [run["training_elapsed_seconds"] for run in selected]
        models[name] = {
            "parameters": sorted({run["protocol"]["parameters"] for run in selected}),
            "test_bits_per_byte": quality,
            "test_bits_per_byte_stats": _stats(quality) if quality else None,
            "steady_training_tokens_per_second_stats": _stats(throughput)
            if throughput
            else None,
            "test_inference_tokens_per_second_stats": _stats(inference)
            if inference
            else None,
            "peak_rss_kib_stats": _stats(peak_rss) if peak_rss else None,
            "training_elapsed_seconds_stats": _stats(elapsed) if elapsed else None,
        }
    revisions = [
        indexed[("sisyphus", seed)]["final_test"]["round_bits_per_byte"][1]
        - indexed[("sisyphus", seed)]["final_test"]["round_bits_per_byte"][0]
        for seed in seeds
    ]
    sisyphus_mean = models["sisyphus"]["test_bits_per_byte_stats"]["mean"]
    transformer_mean = models["transformer"]["test_bits_per_byte_stats"]["mean"]
    sisyphus_train = models["sisyphus"]["steady_training_tokens_per_second_stats"]["mean"]
    transformer_train = models["transformer"]["steady_training_tokens_per_second_stats"]["mean"]
    result = {
        "models": models,
        "paired": {
            "definition": "Sisyphus test bpb minus Transformer test bpb; negative favors Sisyphus",
            "seeds": seeds,
            "differences": differences,
            "stats": _stats(differences) if differences else None,
            "bootstrap_95_percent_ci": interval,
            "sisyphus_wins": sum(value < 0 for value in differences),
            "transformer_wins": sum(value > 0 for value in differences),
            "ties": sum(value == 0 for value in differences),
            "quality_edge_supported": (
                len(seeds) >= 5
                and sum(value < 0 for value in differences) >= 4
                and interval[1] is not None
                and interval[1] < 0
            ),
            "mean_relative_bpb_reduction_percent": (
                (transformer_mean - sisyphus_mean) / transformer_mean * 100.0
                if differences
                else None
            ),
        },
        "recursive_revision": {
            "definition": "round 2 bpb minus round 1 bpb; negative means revision improved prediction",
            "differences": revisions,
            "stats": _stats(revisions) if revisions else None,
            "improved_seeds": sum(value < 0 for value in revisions),
        },
        "efficiency": {
            "sisyphus_to_transformer_training_throughput_ratio": (
                sisyphus_train / transformer_train if transformer_train else None
            ),
            "transformer_training_speedup": (
                transformer_train / sisyphus_train if sisyphus_train else None
            ),
        },
    }
    return result


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

"""Apply the pre-registered H1 falsification rule to the pilot ablation runs.

H1 (``PAPER.md`` section 8.2): H1 is falsified at this scale if the complex
arm fails to beat the real-valued ablation under a paired-bootstrap rule; the
path claim specifically is falsified if the complex arm fails to beat the
non-recursive and phase-destroyed ablations.
"""

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
    }


def _paired(reference: dict, other: dict) -> dict:
    seeds = sorted(set(reference) & set(other))
    differences = [reference[seed] - other[seed] for seed in seeds]
    random = np.random.default_rng(20260720)
    if differences:
        array = np.asarray(differences)
        samples = random.choice(array, size=(100_000, len(array)), replace=True).mean(axis=1)
        interval = [float(value) for value in np.quantile(samples, (0.025, 0.975))]
    else:
        interval = [None, None]
    return {
        "seeds": seeds,
        "differences": differences,
        "stats": _stats(differences) if differences else None,
        "bootstrap_95_percent_ci": interval,
        "reference_wins": sum(value < 0 for value in differences),
        "reference_beats_other": bool(differences) and interval[1] is not None and interval[1] < 0,
    }


def summarize(directory: Path) -> dict:
    runs = [json.loads(path.read_text()) for path in sorted(directory.glob("*.json"))]
    runs = [run for run in runs if run.get("status") == "complete"]
    by_arm: dict[str, dict[int, float]] = {}
    for run in runs:
        arm = run["protocol"]["model"]["arm"]
        seed = run["protocol"]["seed"]
        by_arm.setdefault(arm, {})[seed] = run["final_test"]["bits_per_byte"]

    complex_scores = by_arm.get("complex", {})
    comparisons = {
        other: _paired(complex_scores, by_arm[other])
        for other in ("real", "nonrecursive", "phase-destroyed")
        if other in by_arm
    }
    h1_falsified = "real" in comparisons and not comparisons["real"]["reference_beats_other"]
    path_falsified = (
        "nonrecursive" in comparisons
        and "phase-destroyed" in comparisons
        and not (
            comparisons["nonrecursive"]["reference_beats_other"]
            and comparisons["phase-destroyed"]["reference_beats_other"]
        )
    )
    result = {
        "definition": "complex-arm test bpb minus other-arm test bpb; negative favors complex",
        "arms_present": sorted(by_arm),
        "scores_by_arm": {
            arm: {seed: score for seed, score in sorted(scores.items())}
            for arm, scores in by_arm.items()
        },
        "comparisons": comparisons,
        "h1_falsified_at_this_scale": h1_falsified,
        "path_claim_falsified_at_this_scale": path_falsified,
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

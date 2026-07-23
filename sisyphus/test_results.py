from __future__ import annotations

import hashlib
import json
import unittest
from pathlib import Path

from .improve import fib
from .summarize import summarize
from .summarize_scaling import summarize as summarize_scaling


ROOT = Path(__file__).resolve().parent
IMPLEMENTATION_FILES = ("benchmark_lm.py", "data.py", "layers.py", "models.py")


class RetainedResultTests(unittest.TestCase):
    def test_fibonacci_attempt_schedule(self) -> None:
        self.assertEqual([fib(index) for index in range(8)], [1, 1, 2, 3, 5, 8, 13, 21])

    def test_official_quality_rule_recomputes(self) -> None:
        result = summarize(ROOT / "results" / "enwik8_v1")
        self.assertTrue(result["paired"]["quality_edge_supported"])
        self.assertEqual(result["paired"]["sisyphus_wins"], 4)
        self.assertEqual(result["recursive_revision"]["improved_seeds"], 5)
        self.assertLess(result["paired"]["bootstrap_95_percent_ci"][1], 0.0)

    def test_untouched_confirmation_failure_recomputes(self) -> None:
        result = summarize(ROOT / "results" / "text8_v1")
        self.assertFalse(result["paired"]["quality_edge_supported"])
        self.assertEqual(result["paired"]["sisyphus_wins"], 0)
        self.assertEqual(result["paired"]["transformer_wins"], 5)
        self.assertEqual(result["recursive_revision"]["improved_seeds"], 5)
        self.assertGreater(result["paired"]["bootstrap_95_percent_ci"][0], 0.0)

    def test_retained_records_match_source_and_paired_schedules(self) -> None:
        implementation = {
            name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
            for name in IMPLEMENTATION_FILES
        }
        studies = {
            "enwik8_v1": (
                "2b49720ec4d78c3c9fabaee6e4179a5e997302b3a70029f30f2d582218c024a8",
                (17, 29, 43, 71, 113),
            ),
            "text8_v1": (
                "6e890197040d37d85beb962ae1f041ff1d9a9ca8d20c7d99c85027eebf51dca7",
                (19, 31, 47, 73, 127),
            ),
        }
        for directory, (corpus_hash, seeds) in studies.items():
            paths = sorted((ROOT / "results" / directory).glob("*.seed_*.json"))
            self.assertEqual(len(paths), 10)
            records = {}
            for path in paths:
                record = json.loads(path.read_text())
                protocol = record["protocol"]
                key = (protocol["seed"], protocol["model"]["name"])
                records[key] = protocol
                self.assertEqual(record["status"], "complete")
                self.assertEqual(record["completed_steps"], 500)
                self.assertEqual(protocol["trained_tokens"], 256_000)
                self.assertEqual(protocol["dataset"]["sha256"], corpus_hash)
                self.assertEqual(protocol["implementation_sha256"], implementation)
            for seed in seeds:
                sisyphus = records[(seed, "sisyphus")]
                transformer = records[(seed, "transformer")]
                self.assertEqual(sisyphus["schedule_sha256"], transformer["schedule_sha256"])
                self.assertEqual(sisyphus["data_seed"], transformer["data_seed"])

    def test_scaling_crossover_recomputes(self) -> None:
        result = summarize_scaling(ROOT / "results" / "scaling_v1")
        self.assertEqual(result["measured_inference_crossover_context"], 2048)
        longest = result["pairs"][-1]
        self.assertGreater(longest["sisyphus_speedup"], 5.0)
        self.assertGreater(longest["sisyphus_peak_rss_reduction_percent"], 80.0)


if __name__ == "__main__":
    unittest.main()

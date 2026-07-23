from __future__ import annotations

import unittest

import numpy as np
from tinygrad import Tensor

from .compiler import ensure_compiler
from .models import SisyphusLM, build_model

ensure_compiler()


class SisyphusModelTests(unittest.TestCase):
    def test_shapes_and_finite_loss(self) -> None:
        Tensor.manual_seed(7)
        model = SisyphusLM(
            vocab_size=32, context_length=16, width=12, ffn_hidden=24, rounds=2
        )
        tokens = Tensor(np.arange(32, dtype=np.int32).reshape(2, 16) % 32)
        targets = Tensor((np.arange(32, dtype=np.int32).reshape(2, 16) + 1) % 32)
        self.assertEqual(model(tokens).shape, (2, 16, 32))
        self.assertTrue(np.isfinite(float(model.loss(tokens, targets).item())))

    def test_exact_causality_for_every_round(self) -> None:
        Tensor.manual_seed(11)
        model = SisyphusLM(
            vocab_size=32, context_length=16, width=12, ffn_hidden=24, rounds=2
        )
        base = np.arange(16, dtype=np.int32)[None, :] % 32
        changed = base.copy()
        changed[:, 9:] = (changed[:, 9:] + 13) % 32
        before = [value.numpy() for value in model.logits_by_round(Tensor(base))]
        after = [value.numpy() for value in model.logits_by_round(Tensor(changed))]
        for left, right in zip(before, after):
            np.testing.assert_allclose(left[:, :9], right[:, :9], atol=0.0, rtol=0.0)

    def test_parameter_match_is_within_ten_percent(self) -> None:
        _, _, sisyphus = build_model("sisyphus", context_length=128)
        _, _, transformer = build_model("transformer", context_length=128)
        relative = abs(sisyphus - transformer) / transformer
        self.assertLessEqual(relative, 0.10, (sisyphus, transformer, relative))


if __name__ == "__main__":
    unittest.main()

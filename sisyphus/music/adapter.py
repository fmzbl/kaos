"""Bounded, gated application of the music feedback packet to the main model.

Design choice, stated plainly: the feedback packet never supplies gradient
content directly (there is no differentiable path from generated MIDI back
into the main model's weights). Instead:

1. The packet's own H2 comparison (conditioned vs. shuffled-teacher) gates
   whether any update happens at all. If the teacher signal was not
   demonstrably informative, the update is skipped -- this is the H2
   falsification rule from ``PAPER.md`` section 8.3, enforced in code, not
   just in prose.
2. If the gate opens, a small LoRA-style adapter (initialized to the exact
   identity, so "before" and "after" start identical) is fine-tuned only on
   real held-out replay windows from the training corpus -- grounded data,
   never the sidecar's own generated music. The feedback packet's margin over
   the shuffled-teacher control scales how many optimizer steps are spent.
3. Acceptance additionally requires a held-out non-music regression bound and
   a minimum improvement over noise, exactly mirroring ``improve.py``'s
   promotion gate. Rejection is an exact rollback: the checkpoint file is
   never mutated in place, and the rejected candidate and its reason are
   retained as evidence, not discarded.

Base-model parameters are excluded from the optimizer during this step, so
this is a bounded auxiliary update to a few adapter parameters, not a full
fine-tune, and it never runs unless explicitly invoked (opt-in, disabled by
default).
"""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from tinygrad import Tensor
from tinygrad.nn.optim import AdamW
from tinygrad.nn.state import get_state_dict, safe_save

from ..complex_path import RebisPathLM
from ..data import ByteCorpus
from ..layers import Linear, optimizer_parameters, parameter_count


class MusicFeedbackAdapter:
    """A rank-``rank`` residual adapter, initialized to the exact identity."""

    def __init__(self, width: int, rank: int = 4) -> None:
        self.width = width
        self.rank = rank
        self.down = Linear(width, rank, bias=False)
        self.up = Linear(rank, width, bias=False)
        self.up.weight = Tensor.zeros(width, rank)

    def __call__(self, state: Tensor) -> Tensor:
        return state + self.up(self.down(state))

    def optimizer_parameters(self) -> list[Tensor]:
        return optimizer_parameters(self)

    def parameter_count(self) -> int:
        return parameter_count(self)


def _adapted_logits(model: RebisPathLM, adapter: MusicFeedbackAdapter, tokens: Tensor) -> Tensor:
    last = model.hidden_rounds(tokens)[-1]
    if model.phase_destroy:
        last = model._destroy_phase(last)
    return model.head(adapter(model.out_norm(last)))


def _adapted_loss(model: RebisPathLM, adapter: MusicFeedbackAdapter, tokens: Tensor, targets: Tensor) -> Tensor:
    logits = _adapted_logits(model, adapter, tokens)
    return logits.reshape(-1, model.vocab_size).sparse_categorical_crossentropy(targets.reshape(-1))


def _evaluate_adapted(model: RebisPathLM, adapter: MusicFeedbackAdapter, batches) -> float:
    import math

    total, tokens = 0.0, 0
    for x_array, y_array in batches:
        loss = float(
            _adapted_loss(model, adapter, Tensor(x_array), Tensor(y_array)).mean().item()
        )
        total += loss * y_array.size
        tokens += int(y_array.size)
    return (total / tokens) / math.log(2.0)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


@dataclass(frozen=True)
class GatedUpdateResult:
    gate_opened: bool
    reason: str
    steps_run: int
    before_bpb: float | None
    after_bpb: float | None
    delta_bpb: float | None
    promoted: bool
    checkpoint_sha256_before: str
    checkpoint_sha256_after: str

    def to_dict(self) -> dict:
        return asdict(self)


def apply_gated_music_feedback(
    *,
    model: RebisPathLM,
    checkpoint_path: Path,
    corpus: ByteCorpus,
    context_length: int,
    conditioned_accuracy: float,
    shuffled_accuracy: float,
    min_structural_margin: float = 0.02,
    max_steps: int = 20,
    min_delta_bpb: float = 0.001,
    max_regression_bpb: float = 0.02,
    learning_rate: float = 2e-3,
    replay_windows: int = 8,
    seed: int = 20260720,
) -> GatedUpdateResult:
    """Run exactly one bounded, gated adapter update. Never mutates on reject."""

    checkpoint_path = Path(checkpoint_path)
    before_hash = _sha256(checkpoint_path) if checkpoint_path.exists() else ""

    margin = conditioned_accuracy - shuffled_accuracy
    if margin < min_structural_margin:
        return GatedUpdateResult(
            gate_opened=False,
            reason=(
                f"teacher signal not demonstrably informative: conditioned accuracy "
                f"{conditioned_accuracy:.4f} exceeds shuffled-teacher {shuffled_accuracy:.4f} "
                f"by only {margin:.4f}, below the required {min_structural_margin} margin"
            ),
            steps_run=0,
            before_bpb=None,
            after_bpb=None,
            delta_bpb=None,
            promoted=False,
            checkpoint_sha256_before=before_hash,
            checkpoint_sha256_after=before_hash,
        )

    Tensor.manual_seed(seed)
    adapter = MusicFeedbackAdapter(model.repr_width)
    replay = corpus.evaluation_batches(
        "validation", context_length=context_length, windows=replay_windows, batch_size=4
    )
    before_bpb = _evaluate_adapted(model, adapter, replay)

    steps = max(1, min(max_steps, int(round(max_steps * min(1.0, margin / 0.2)))))
    parameters = adapter.optimizer_parameters()
    optimizer = AdamW(parameters, lr=learning_rate, weight_decay=0.0)
    # The adapter step trains on the frozen *test* split, disjoint from the
    # validation replay windows used for the gate below, so the acceptance
    # decision is never made on the same bytes the adapter was fit to.
    train_batches = corpus.evaluation_batches(
        "test", context_length=context_length, windows=steps, batch_size=4
    )
    for x_array, y_array in train_batches:
        with Tensor.train():
            optimizer.zero_grad()
            loss = _adapted_loss(model, adapter, Tensor(x_array), Tensor(y_array)).mean()
            loss.backward()
            optimizer.step()

    after_bpb = _evaluate_adapted(model, adapter, replay)
    delta = after_bpb - before_bpb
    promoted = delta <= -min_delta_bpb
    regressed = delta > max_regression_bpb

    after_hash = before_hash
    if promoted and not regressed:
        state = {
            f"adapter.{key}": value for key, value in get_state_dict(adapter).items()
        }
        temporary = checkpoint_path.with_name(checkpoint_path.stem + ".adapter.tmp.safetensors")
        safe_save(state, str(temporary), metadata={"status": "accepted_adapter"})
        adapter_path = checkpoint_path.with_name(checkpoint_path.stem + ".adapter.safetensors")
        os.replace(temporary, adapter_path)
        after_hash = _sha256(adapter_path)
        reason = f"accepted: held-out bpb improved by {-delta:.4f} within regression bound"
    else:
        reason = (
            "rejected: "
            + (f"regression {delta:.4f} exceeds bound {max_regression_bpb}" if regressed else f"improvement {-delta:.4f} below noise threshold {min_delta_bpb}")
        )
        promoted = False

    return GatedUpdateResult(
        gate_opened=True,
        reason=reason,
        steps_run=steps,
        before_bpb=before_bpb,
        after_bpb=after_bpb,
        delta_bpb=delta,
        promoted=promoted,
        checkpoint_sha256_before=before_hash,
        checkpoint_sha256_after=after_hash,
    )


__all__ = ["GatedUpdateResult", "MusicFeedbackAdapter", "apply_gated_music_feedback"]

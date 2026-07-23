"""The music sidecar: a small, separately optimized student model.

It conditions a symbolic composition over frozen teacher features (a single
real vector out of ``teacher.TeacherExport.feature_vector``) and predicts the
deterministic target produced by ``target.teacher_to_score_and_targets``.
Its parameters, optimizer, and checkpoint are entirely separate from the main
model's; nothing here ever receives a main-model gradient.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass

import numpy as np
from tinygrad import Tensor

from ..layers import Linear, RMSNorm, optimizer_parameters, parameter_count
from .target import DURATION_BUCKETS, PITCH_BUCKETS, VELOCITY_BUCKETS


@dataclass(frozen=True)
class SidecarSpec:
    feature_width: int
    hidden_width: int
    voices: int
    max_steps: int
    max_rounds: int

    def to_dict(self) -> dict:
        return asdict(self)


class SidecarModel:
    """A tiny recurrent student conditioned on a frozen teacher vector."""

    def __init__(
        self,
        *,
        feature_width: int,
        hidden_width: int = 16,
        voices: int = 2,
        max_steps: int = 64,
        max_rounds: int = 8,
    ) -> None:
        self.feature_width = feature_width
        self.hidden_width = hidden_width
        self.voices = voices
        self.max_steps = max_steps
        self.max_rounds = max_rounds
        self.condition = Linear(feature_width, hidden_width, bias=True)
        self.recurrent = Linear(hidden_width, hidden_width, bias=False)
        self.step_code = Tensor.randn(max_steps, hidden_width) * 0.02
        self.norm = RMSNorm(hidden_width)
        self.pitch_head = Linear(hidden_width, PITCH_BUCKETS, bias=True)
        self.duration_head = Linear(hidden_width, DURATION_BUCKETS, bias=True)
        self.velocity_head = Linear(hidden_width, VELOCITY_BUCKETS, bias=True)
        self.voice_head = Linear(hidden_width, voices, bias=True)
        self.harmony_root_head = Linear(hidden_width, 12, bias=True)
        self.harmony_quality_head = Linear(hidden_width, 4, bias=True)
        self.tempo_head = Linear(hidden_width, 16, bias=True)

    def unroll(self, feature_vector: Tensor, steps: int, levels: int) -> list[Tensor]:
        """Return the per-step hidden state for ``steps`` composition slots."""

        if steps > self.max_steps:
            raise ValueError("steps exceeds sidecar max_steps cap")
        hidden = self.condition(feature_vector).tanh()
        states = []
        for step in range(steps):
            code = self.step_code[step].reshape(1, self.hidden_width)
            hidden = self.norm(self.recurrent(hidden) + code + hidden).tanh()
            states.append(hidden)
        return states

    def predict(self, feature_vector: Tensor, steps: int, levels: int, rounds: int) -> dict:
        states = self.unroll(feature_vector, steps, levels)
        stacked = Tensor.stack(*states, dim=1)  # (batch, steps, hidden)
        pitch_logits = self.pitch_head(stacked)
        duration_logits = self.duration_head(stacked)
        velocity_logits = self.velocity_head(stacked)
        voice_logits = self.voice_head(stacked)

        if rounds > self.max_rounds:
            raise ValueError("rounds exceeds sidecar max_rounds cap")
        round_states = []
        for round_index in range(rounds):
            start, end = round_index * levels, (round_index + 1) * levels
            round_states.append(
                sum(states[start:end], Tensor.zeros_like(states[0])) / max(1, end - start)
            )
        round_stacked = Tensor.stack(*round_states, dim=1)
        harmony_root_logits = self.harmony_root_head(round_stacked)
        harmony_quality_logits = self.harmony_quality_head(round_stacked)
        tempo_logits = self.tempo_head(states[-1])
        return {
            "pitch": pitch_logits,
            "duration": duration_logits,
            "velocity": velocity_logits,
            "voice": voice_logits,
            "harmony_root": harmony_root_logits,
            "harmony_quality": harmony_quality_logits,
            "tempo": tempo_logits,
        }

    def optimizer_parameters(self) -> list[Tensor]:
        return optimizer_parameters(self)

    def parameter_count(self) -> int:
        return parameter_count(self)


def loss_against_targets(predictions: dict, targets, steps: int, rounds: int) -> Tensor:
    def _term(logits: Tensor, labels: np.ndarray) -> Tensor:
        flat_logits = logits.reshape(-1, logits.shape[-1])
        flat_labels = Tensor(labels.astype("int32")).reshape(-1)
        return flat_logits.sparse_categorical_crossentropy(flat_labels)

    pitch = _term(predictions["pitch"], targets.pitch_bucket[None, :steps])
    duration = _term(predictions["duration"], targets.duration_bucket[None, :steps])
    velocity = _term(predictions["velocity"], targets.velocity_bucket[None, :steps])
    voice = _term(predictions["voice"], targets.voice[None, :steps])
    harmony_root = _term(predictions["harmony_root"], targets.harmony_root[None, :rounds])
    harmony_quality = _term(predictions["harmony_quality"], targets.harmony_quality[None, :rounds])
    tempo = predictions["tempo"].reshape(-1, predictions["tempo"].shape[-1]).sparse_categorical_crossentropy(
        Tensor(np.asarray([targets.tempo_bucket], dtype="int32"))
    )
    return (pitch + duration + velocity + voice + harmony_root + harmony_quality + tempo) / 7.0


__all__ = ["SidecarModel", "SidecarSpec", "loss_against_targets"]

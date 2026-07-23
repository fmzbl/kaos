"""Frozen, versioned teacher export at a checkpoint boundary.

This is the *only* place the music sidecar reads the main model. It exports
exactly the representations named in the contract -- hidden states, logits,
and route/operator traces -- as plain NumPy arrays with no residual
computation graph, so no gradient can ever flow from the sidecar back into
the main model through this object. The export is hashed and versioned so a
sidecar training run can prove which exact checkpoint and which exact fixed
input windows produced its teacher signal.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from tinygrad import Tensor

from ..complex_path import RebisPathLM

TEACHER_EXPORT_VERSION = "music-sidecar-teacher-v1"


@dataclass(frozen=True)
class TeacherExport:
    version: str
    source_model: str
    source_arm: str
    source_config_sha256: str
    fixed_tokens_sha256: str
    round_radius_mean: list[float]
    round_radius_std: list[float]
    round_phase_circular_mean: list[float]
    round_phase_std: list[float]
    operator_weights_mean: list[list[float]]  # one 4-vector per (round, level)
    logit_entropy_by_round: list[float]

    def to_dict(self) -> dict:
        return {
            "version": self.version,
            "source_model": self.source_model,
            "source_arm": self.source_arm,
            "source_config_sha256": self.source_config_sha256,
            "fixed_tokens_sha256": self.fixed_tokens_sha256,
            "round_radius_mean": self.round_radius_mean,
            "round_radius_std": self.round_radius_std,
            "round_phase_circular_mean": self.round_phase_circular_mean,
            "round_phase_std": self.round_phase_std,
            "operator_weights_mean": self.operator_weights_mean,
            "logit_entropy_by_round": self.logit_entropy_by_round,
        }

    def feature_vector(self) -> np.ndarray:
        """A single fixed-width real vector the sidecar conditions on."""

        operator_flat = np.asarray(self.operator_weights_mean, dtype=np.float64).reshape(-1)
        return np.concatenate(
            [
                np.asarray(self.round_radius_mean, dtype=np.float64),
                np.asarray(self.round_radius_std, dtype=np.float64),
                np.asarray(self.round_phase_circular_mean, dtype=np.float64),
                np.asarray(self.round_phase_std, dtype=np.float64),
                operator_flat,
                np.asarray(self.logit_entropy_by_round, dtype=np.float64),
            ]
        ).astype(np.float32)


def _entropy(logits: np.ndarray) -> float:
    shifted = logits - logits.max(axis=-1, keepdims=True)
    exponentials = np.exp(shifted)
    probabilities = exponentials / exponentials.sum(axis=-1, keepdims=True)
    return float(-(probabilities * np.log(probabilities + 1e-12)).sum(axis=-1).mean())


def export_teacher(
    model: RebisPathLM, fixed_tokens: np.ndarray, *, source_config_sha256: str
) -> TeacherExport:
    """Run one frozen forward pass and detach everything to NumPy."""

    if fixed_tokens.ndim != 2:
        raise ValueError("fixed_tokens must be a (batch, length) array")
    trace: list = []
    rounds = model.hidden_rounds(Tensor(fixed_tokens.astype("int32")), trace=trace)
    logits = model.logits_by_round(Tensor(fixed_tokens.astype("int32")))

    radius_mean, radius_std, phase_mean, phase_std = [], [], [], []
    for state in rounds:
        array = state.numpy()
        a, b = array[..., : model.width], array[..., model.width :]
        magnitude = np.sqrt(a**2 + b**2)
        radius_mean.append(float(magnitude.mean()))
        radius_std.append(float(magnitude.std()))
        if model.complex_mode:
            phase = np.arctan2(b, a)
            phase_mean.append(float(np.angle(np.mean(np.exp(1j * phase)))))
            phase_std.append(float(np.std(phase)))
        else:
            phase_mean.append(0.0)
            phase_std.append(0.0)

    operator_weights = [entry["operator_weights_mean"] for entry in trace]
    entropy_by_round = [_entropy(value.numpy()) for value in logits]
    fixed_tokens_sha256 = hashlib.sha256(np.ascontiguousarray(fixed_tokens).tobytes()).hexdigest()

    return TeacherExport(
        version=TEACHER_EXPORT_VERSION,
        source_model=type(model).__name__,
        source_arm=model.arm,
        source_config_sha256=source_config_sha256,
        fixed_tokens_sha256=fixed_tokens_sha256,
        round_radius_mean=radius_mean,
        round_radius_std=radius_std,
        round_phase_circular_mean=phase_mean,
        round_phase_std=phase_std,
        operator_weights_mean=operator_weights,
        logit_entropy_by_round=entropy_by_round,
    )


def write_teacher_export(export: TeacherExport, path: str | Path) -> Path:
    resolved = Path(path).expanduser().resolve()
    resolved.parent.mkdir(parents=True, exist_ok=True)
    resolved.write_text(json.dumps(export.to_dict(), indent=2, sort_keys=True) + "\n")
    return resolved


def read_teacher_export(path: str | Path) -> TeacherExport:
    payload = json.loads(Path(path).expanduser().resolve().read_text())
    return TeacherExport(**payload)


__all__ = [
    "TEACHER_EXPORT_VERSION",
    "TeacherExport",
    "export_teacher",
    "read_teacher_export",
    "write_teacher_export",
]

"""Decode sidecar predictions into artifacts, and compute the feedback packet.

The feedback packet is the *only* channel back toward the main model (via
``adapter.py``). It is intentionally compact and auditable: a handful of
named scalars with a clear provenance trail, not raw activations and not a
copy of the generated music.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass

import numpy as np

from .events import MAX_DURATION_TICKS, HARMONY_QUALITIES, HarmonyEvent, NoteEvent, PPQ, Score, SynthControl
from .target import VELOCITY_BUCKETS, TargetLabels
from .teacher import TeacherExport

# A raw (especially untrained or adversarial) sidecar prediction can argmax to
# any of the DURATION_BUCKETS classes, including ones that would encode a
# single note far longer than any target ever uses. Hard caps must hold for
# every possible model output, not just a well-trained one, so decoding caps
# the exponent independently of validate_score's after-the-fact check.
SAFE_DURATION_BUCKET_EXPONENT_CAP = 6  # (PPQ // 16) * 2**6 = 1,920 ticks per note


def decode_score(predictions: dict, *, voices: int, levels: int, tempo_bpm: float) -> Score:
    """Deterministically argmax-decode the sidecar's own predictions.

    Always returns a score that satisfies ``events.validate_score``'s hard
    caps, regardless of how the sidecar was trained: per-note duration is
    capped, and a voice's timeline stops growing new notes once it reaches
    the total-duration cap instead of raising.
    """

    pitch = predictions["pitch"].numpy().argmax(-1)[0]
    duration_bucket = predictions["duration"].numpy().argmax(-1)[0]
    velocity_bucket = predictions["velocity"].numpy().argmax(-1)[0]
    voice = predictions["voice"].numpy().argmax(-1)[0]
    harmony_root = predictions["harmony_root"].numpy().argmax(-1)[0]
    harmony_quality = predictions["harmony_quality"].numpy().argmax(-1)[0]

    cursor = [0] * voices
    notes = []
    for index in range(len(pitch)):
        this_voice = int(voice[index]) % voices
        exponent = min(int(duration_bucket[index]), SAFE_DURATION_BUCKET_EXPONENT_CAP)
        duration_ticks = int((PPQ // 16) * (2**exponent))
        start_tick = cursor[this_voice]
        if start_tick >= MAX_DURATION_TICKS:
            continue  # this voice's timeline already reached the hard cap
        duration_ticks = min(duration_ticks, MAX_DURATION_TICKS - start_tick)
        velocity = int(1 + velocity_bucket[index] * (126 // max(1, VELOCITY_BUCKETS - 1)))
        notes.append(
            NoteEvent(
                voice=this_voice,
                pitch=int(np.clip(pitch[index], 0, 127)),
                velocity=int(np.clip(velocity, 1, 127)),
                start_tick=start_tick,
                duration_ticks=duration_ticks,
            )
        )
        cursor[this_voice] += duration_ticks

    harmony_events = [
        HarmonyEvent(
            start_tick=round_index * PPQ * 4,
            root_pitch_class=int(harmony_root[round_index]) % 12,
            quality=HARMONY_QUALITIES[int(harmony_quality[round_index]) % 4],
        )
        for round_index in range(len(harmony_root))
    ]
    synth = {
        voice_index: SynthControl(
            oscillator=("sine", "triangle", "saw", "square")[voice_index % 4],
            attack_ms=10.0,
            decay_ms=40.0,
            sustain_level=0.6,
            release_ms=120.0,
            filter_cutoff_hz=4000.0,
            gain=0.7,
            pan=-0.5 if voice_index % 2 == 0 else 0.5,
        )
        for voice_index in range(voices)
    }
    return Score(tempo_bpm=tempo_bpm, voices=voices, notes=tuple(notes), harmony=tuple(harmony_events), synth=synth)


@dataclass(frozen=True)
class FeedbackPacket:
    source_config_sha256: str
    fixed_tokens_sha256: str
    control: str  # "conditioned", "shuffled-teacher", or "unconditioned"
    structural_prediction_accuracy: float
    field_accuracy: dict
    temporal_hierarchy_score: float
    phase_rhythm_alignment: float
    velocity_operator_correlation: float

    def to_dict(self) -> dict:
        return asdict(self)


def _safe_corrcoef(left: np.ndarray, right: np.ndarray) -> float:
    if left.std() == 0 or right.std() == 0:
        return 0.0
    return float(np.corrcoef(left, right)[0, 1])


def compute_feedback_packet(
    predictions: dict,
    targets: TargetLabels,
    export: TeacherExport,
    *,
    levels: int,
    control: str,
) -> FeedbackPacket:
    pitch_pred = predictions["pitch"].numpy().argmax(-1)[0]
    duration_pred = predictions["duration"].numpy().argmax(-1)[0]
    velocity_pred = predictions["velocity"].numpy().argmax(-1)[0]
    voice_pred = predictions["voice"].numpy().argmax(-1)[0]
    harmony_root_pred = predictions["harmony_root"].numpy().argmax(-1)[0]
    harmony_quality_pred = predictions["harmony_quality"].numpy().argmax(-1)[0]

    steps = len(pitch_pred)
    rounds = len(harmony_root_pred)
    field_accuracy = {
        "pitch": float((pitch_pred == targets.pitch_bucket[:steps]).mean()),
        "duration": float((duration_pred == targets.duration_bucket[:steps]).mean()),
        "velocity": float((velocity_pred == targets.velocity_bucket[:steps]).mean()),
        "voice": float((voice_pred == targets.voice[:steps]).mean()),
        "harmony_root": float((harmony_root_pred == targets.harmony_root[:rounds]).mean()),
        "harmony_quality": float((harmony_quality_pred == targets.harmony_quality[:rounds]).mean()),
    }
    structural_prediction_accuracy = float(np.mean(list(field_accuracy.values())))

    level_index = np.arange(steps) % levels
    temporal_hierarchy_score = _safe_corrcoef(duration_pred.astype(np.float64), level_index.astype(np.float64))

    phase_signal = np.asarray(export.round_phase_circular_mean[:rounds], dtype=np.float64)
    phase_rhythm_alignment = _safe_corrcoef(harmony_root_pred.astype(np.float64), phase_signal)

    square_weight_signal = np.asarray(
        [entry[3] for entry in export.operator_weights_mean[:steps]], dtype=np.float64
    )
    velocity_operator_correlation = _safe_corrcoef(velocity_pred.astype(np.float64), square_weight_signal)

    return FeedbackPacket(
        source_config_sha256=export.source_config_sha256,
        fixed_tokens_sha256=export.fixed_tokens_sha256,
        control=control,
        structural_prediction_accuracy=structural_prediction_accuracy,
        field_accuracy=field_accuracy,
        temporal_hierarchy_score=temporal_hierarchy_score,
        phase_rhythm_alignment=phase_rhythm_alignment,
        velocity_operator_correlation=velocity_operator_correlation,
    )


__all__ = ["FeedbackPacket", "compute_feedback_packet", "decode_score"]

"""A fixed, non-learned map from frozen teacher features to a music target.

Self-generated music must never certify itself (``PAPER.md`` section 3's rule
against ungrounded self-training carried into H2). This module is the
alternative source of ground truth: a deterministic, documented, arbitrary-
but-fixed function of the teacher's own frozen structural signals (dyadic
route offsets, radius, phase, operator-mixture weights). The sidecar is then
trained to *predict* this target from a masked/shuffled view of the teacher
vector -- a real distillation problem with a real answer key, not a taste
judgment and not the sidecar grading its own homework.

The specific constants below are an arbitrary engineering choice, not a
discovered musical law; ``PAPER.md`` states this plainly and never reports the
resulting scores as evidence of musicality.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

import numpy as np

from .events import HARMONY_QUALITIES, HarmonyEvent, NoteEvent, PPQ, Score, SynthControl
from .teacher import TeacherExport

DURATION_BUCKETS = 16
VELOCITY_BUCKETS = 8
PITCH_BUCKETS = 128


@dataclass(frozen=True)
class TargetLabels:
    pitch_bucket: np.ndarray
    duration_bucket: np.ndarray
    velocity_bucket: np.ndarray
    voice: np.ndarray
    harmony_root: np.ndarray
    harmony_quality: np.ndarray
    tempo_bucket: int


def _clamp(value: float, low: float, high: float) -> float:
    return max(low, min(high, value))


def teacher_to_score_and_targets(
    export: TeacherExport, *, voices: int = 2
) -> tuple[Score, TargetLabels]:
    rounds = len(export.round_radius_mean)
    entries = export.operator_weights_mean
    levels = max(1, len(entries) // max(1, rounds))

    tempo_route_weight = float(np.mean([entry[1] for entry in entries])) if entries else 0.5
    tempo_bpm = _clamp(60.0 + 100.0 * tempo_route_weight, 40.0, 220.0)
    tempo_bucket = int(round(tempo_route_weight * 15))

    pitch_bucket = np.zeros(len(entries), dtype=np.int64)
    duration_bucket = np.zeros(len(entries), dtype=np.int64)
    velocity_bucket = np.zeros(len(entries), dtype=np.int64)
    voice = np.zeros(len(entries), dtype=np.int64)
    notes: list[NoteEvent] = []
    cursor = [0] * voices

    for index, entry in enumerate(entries):
        round_index = index // levels
        level = index % levels
        offset = 1 << level
        radius = export.round_radius_mean[min(round_index, rounds - 1)]
        square_weight = entry[3]  # operator order: quote, route, group, square

        pitch = int(round(_clamp(60.0 + radius * 12.0, 36.0, 96.0)))
        duration_ticks = int(_clamp(offset * (PPQ // 16), PPQ // 16, PPQ * 8))
        velocity = int(round(_clamp(square_weight, 0.0, 1.0) * 125.0)) + 1
        this_voice = round_index % voices

        pitch_bucket[index] = pitch
        duration_bucket[index] = min(
            DURATION_BUCKETS - 1, int(math.log2(max(1, duration_ticks // (PPQ // 16))))
        )
        velocity_bucket[index] = min(VELOCITY_BUCKETS - 1, velocity * VELOCITY_BUCKETS // 128)
        voice[index] = this_voice

        notes.append(
            NoteEvent(
                voice=this_voice,
                pitch=pitch,
                velocity=velocity,
                start_tick=cursor[this_voice],
                duration_ticks=duration_ticks,
            )
        )
        cursor[this_voice] += duration_ticks

    harmony_root = np.zeros(rounds, dtype=np.int64)
    harmony_quality = np.zeros(rounds, dtype=np.int64)
    harmony_events = []
    for round_index in range(rounds):
        phase = export.round_phase_circular_mean[round_index]
        spread = export.round_phase_std[round_index]
        root = int(round(((phase + math.pi) / (2.0 * math.pi)) * 12.0)) % 12
        quality_index = min(3, int(_clamp(spread, 0.0, 3.999)))
        harmony_root[round_index] = root
        harmony_quality[round_index] = quality_index
        harmony_events.append(
            HarmonyEvent(
                start_tick=round_index * PPQ * 4,
                root_pitch_class=root,
                quality=HARMONY_QUALITIES[quality_index],
            )
        )

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

    score = Score(
        tempo_bpm=tempo_bpm,
        voices=voices,
        notes=tuple(notes),
        harmony=tuple(harmony_events),
        synth=synth,
    )
    labels = TargetLabels(
        pitch_bucket=pitch_bucket,
        duration_bucket=duration_bucket,
        velocity_bucket=velocity_bucket,
        voice=voice,
        harmony_root=harmony_root,
        harmony_quality=harmony_quality,
        tempo_bucket=tempo_bucket,
    )
    return score, labels


__all__ = ["DURATION_BUCKETS", "PITCH_BUCKETS", "VELOCITY_BUCKETS", "TargetLabels", "teacher_to_score_and_targets"]

"""Explicit symbolic event vocabulary for the music sidecar.

A :class:`Score` represents notes, rests, duration, velocity, voice, tempo,
harmony, and synthesizer/instrument controls explicitly, as required by the
sidecar contract. It is a plain, validated, JSON-serializable data structure
so every rendered artifact is auditable independent of any model.
"""

from __future__ import annotations

from dataclasses import asdict, dataclass, field

# Hard caps. These bound every sidecar artifact regardless of configuration;
# they are not tunable via the CLI so a misconfiguration cannot silently grow
# an "opt-in, bounded" sidecar into an unbounded one.
MAX_EVENTS = 512
MAX_VOICES = 8
MAX_DURATION_TICKS = 64 * 480  # 64 quarter notes at the fixed PPQ below
PPQ = 480  # ticks per quarter note, fixed and not configurable
OSCILLATORS = ("sine", "saw", "square", "triangle")
HARMONY_QUALITIES = ("major", "minor", "diminished", "augmented")


class InvalidScoreError(ValueError):
    pass


@dataclass(frozen=True)
class NoteEvent:
    voice: int
    pitch: int  # MIDI pitch, 0-127
    velocity: int  # 1-127
    start_tick: int
    duration_ticks: int

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass(frozen=True)
class RestEvent:
    voice: int
    start_tick: int
    duration_ticks: int

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass(frozen=True)
class HarmonyEvent:
    start_tick: int
    root_pitch_class: int  # 0-11
    quality: str

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass(frozen=True)
class SynthControl:
    oscillator: str
    attack_ms: float
    decay_ms: float
    sustain_level: float  # 0-1
    release_ms: float
    filter_cutoff_hz: float
    gain: float  # 0-1
    pan: float  # -1..1

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass(frozen=True)
class Score:
    tempo_bpm: float
    voices: int
    notes: tuple[NoteEvent, ...] = ()
    rests: tuple[RestEvent, ...] = ()
    harmony: tuple[HarmonyEvent, ...] = ()
    synth: dict[int, SynthControl] = field(default_factory=dict)
    ppq: int = PPQ

    def to_dict(self) -> dict:
        return {
            "tempo_bpm": self.tempo_bpm,
            "voices": self.voices,
            "ppq": self.ppq,
            "notes": [note.to_dict() for note in self.notes],
            "rests": [rest.to_dict() for rest in self.rests],
            "harmony": [event.to_dict() for event in self.harmony],
            "synth": {str(voice): control.to_dict() for voice, control in self.synth.items()},
        }

    def total_ticks(self) -> int:
        ends = [note.start_tick + note.duration_ticks for note in self.notes]
        ends += [rest.start_tick + rest.duration_ticks for rest in self.rests]
        return max(ends, default=0)

    def event_count(self) -> int:
        return len(self.notes) + len(self.rests) + len(self.harmony)


def validate_score(score: Score) -> None:
    """Raise :class:`InvalidScoreError` on any violation. Never silently clamps."""

    if not (20.0 <= score.tempo_bpm <= 300.0):
        raise InvalidScoreError(f"tempo_bpm out of range: {score.tempo_bpm}")
    if not (1 <= score.voices <= MAX_VOICES):
        raise InvalidScoreError(f"voices out of range: {score.voices}")
    if score.event_count() > MAX_EVENTS:
        raise InvalidScoreError(f"event_count {score.event_count()} exceeds cap {MAX_EVENTS}")
    if score.total_ticks() > MAX_DURATION_TICKS:
        raise InvalidScoreError(
            f"total_ticks {score.total_ticks()} exceeds cap {MAX_DURATION_TICKS}"
        )
    for note in score.notes:
        if not (0 <= note.pitch <= 127):
            raise InvalidScoreError(f"invalid pitch {note.pitch}")
        if not (1 <= note.velocity <= 127):
            raise InvalidScoreError(f"invalid velocity {note.velocity}")
        if note.duration_ticks <= 0:
            raise InvalidScoreError("non-positive note duration")
        if note.start_tick < 0:
            raise InvalidScoreError("negative start tick")
        if not (0 <= note.voice < score.voices):
            raise InvalidScoreError(f"voice {note.voice} outside declared {score.voices}")
    for rest in score.rests:
        if rest.duration_ticks <= 0:
            raise InvalidScoreError("non-positive rest duration")
        if rest.start_tick < 0:
            raise InvalidScoreError("negative rest start tick")
        if not (0 <= rest.voice < score.voices):
            raise InvalidScoreError(f"voice {rest.voice} outside declared {score.voices}")
    for event in score.harmony:
        if not (0 <= event.root_pitch_class <= 11):
            raise InvalidScoreError(f"invalid root pitch class {event.root_pitch_class}")
        if event.quality not in HARMONY_QUALITIES:
            raise InvalidScoreError(f"invalid harmony quality {event.quality}")
        if event.start_tick < 0:
            raise InvalidScoreError("negative harmony start tick")
    for voice, control in score.synth.items():
        if not (0 <= voice < score.voices):
            raise InvalidScoreError(f"synth control voice {voice} outside declared {score.voices}")
        if control.oscillator not in OSCILLATORS:
            raise InvalidScoreError(f"invalid oscillator {control.oscillator}")
        if not (0.0 <= control.sustain_level <= 1.0):
            raise InvalidScoreError("sustain_level out of range")
        if not (0.0 <= control.gain <= 1.0):
            raise InvalidScoreError("gain out of range")
        if not (-1.0 <= control.pan <= 1.0):
            raise InvalidScoreError("pan out of range")
        if control.filter_cutoff_hz <= 0:
            raise InvalidScoreError("non-positive filter_cutoff_hz")
        if min(control.attack_ms, control.decay_ms, control.release_ms) < 0:
            raise InvalidScoreError("negative envelope stage")


__all__ = [
    "HARMONY_QUALITIES",
    "HarmonyEvent",
    "InvalidScoreError",
    "MAX_DURATION_TICKS",
    "MAX_EVENTS",
    "MAX_VOICES",
    "NoteEvent",
    "OSCILLATORS",
    "PPQ",
    "RestEvent",
    "Score",
    "SynthControl",
    "validate_score",
]

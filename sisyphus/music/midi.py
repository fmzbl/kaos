"""A minimal, dependency-free, deterministic Standard MIDI File (SMF) writer.

Only the subset of MIDI needed to render a :class:`~sisyphus.music.events.Score`
is implemented: note on/off, tempo, and a per-voice program/pan/volume
preamble derived from the synth controls (a real synthesizer would read the
controls directly; MIDI has no filter/envelope messages, so those controls
are carried verbatim in the sidecar JSON artifact instead and are also used
directly by ``synth.render_wav``).
"""

from __future__ import annotations

import struct

from .events import OSCILLATORS, Score, validate_score

_OSCILLATOR_PROGRAM = {
    "sine": 0,  # acoustic grand piano-ish placeholder program numbers; the
    "triangle": 73,  # actual timbre for the offline path comes from
    "square": 80,  # synth.render_wav, not from a GM soundfont.
    "saw": 81,
}


def _variable_length(value: int) -> bytes:
    if value < 0:
        raise ValueError("negative delta time")
    buffer = [value & 0x7F]
    value >>= 7
    while value:
        buffer.append((value & 0x7F) | 0x80)
        value >>= 7
    return bytes(reversed(buffer))


def _track_chunk(events: list[tuple[int, bytes]]) -> bytes:
    events = sorted(events, key=lambda item: item[0])
    body = bytearray()
    previous = 0
    for absolute_tick, message in events:
        body += _variable_length(absolute_tick - previous)
        body += message
        previous = absolute_tick
    body += _variable_length(0) + b"\xff\x2f\x00"  # end of track
    return b"MTrk" + struct.pack(">I", len(body)) + bytes(body)


def score_to_midi_bytes(score: Score) -> bytes:
    """Deterministically render ``score`` to Standard MIDI File bytes."""

    validate_score(score)
    microseconds_per_quarter = round(60_000_000 / score.tempo_bpm)
    tempo_track = _track_chunk(
        [(0, b"\xff\x51\x03" + microseconds_per_quarter.to_bytes(3, "big"))]
    )
    tracks = [tempo_track]
    for voice in range(score.voices):
        control = score.synth.get(voice)
        program = _OSCILLATOR_PROGRAM.get(control.oscillator, 0) if control else 0
        pan_value = int(round(((control.pan if control else 0.0) + 1.0) * 63.5))
        volume_value = int(round((control.gain if control else 0.8) * 127))
        channel = voice % 16
        events: list[tuple[int, bytes]] = [
            (0, bytes([0xC0 | channel, program])),
            (0, bytes([0xB0 | channel, 10, max(0, min(127, pan_value))])),
            (0, bytes([0xB0 | channel, 7, max(0, min(127, volume_value))])),
        ]
        for note in score.notes:
            if note.voice != voice:
                continue
            events.append((note.start_tick, bytes([0x90 | channel, note.pitch, note.velocity])))
            events.append(
                (note.start_tick + note.duration_ticks, bytes([0x80 | channel, note.pitch, 0]))
            )
        tracks.append(_track_chunk(events))

    header = b"MThd" + struct.pack(">IHHH", 6, 1, len(tracks), score.ppq)
    return header + b"".join(tracks)


__all__ = ["OSCILLATORS", "score_to_midi_bytes"]

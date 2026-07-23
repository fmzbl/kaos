"""Deterministic offline WAV rendering from a validated :class:`Score`.

Pure NumPy plus the standard-library :mod:`wave` module -- no optional
dependency is required, so this path is always available locally, unlike an
external synthesizer. Every oscillator, envelope, and filter stage is a
closed-form deterministic function of the score, so re-rendering the same
score byte-for-byte reproduces the same WAV file.
"""

from __future__ import annotations

import wave
from pathlib import Path

import numpy as np

from .events import Score, validate_score

SAMPLE_RATE = 22_050
MAX_RENDER_SECONDS = 60.0
PEAK_AMPLITUDE = 0.98  # headroom so the final int16 quantization never clips


def _oscillator(kind: str, phase: np.ndarray) -> np.ndarray:
    if kind == "sine":
        return np.sin(2.0 * np.pi * phase)
    if kind == "triangle":
        return 2.0 * np.abs(2.0 * (phase % 1.0) - 1.0) - 1.0
    if kind == "square":
        return np.sign(np.sin(2.0 * np.pi * phase))
    if kind == "saw":
        return 2.0 * (phase % 1.0) - 1.0
    raise ValueError(f"unknown oscillator {kind!r}")


def _envelope(length: int, attack: int, decay: int, sustain_level: float, release: int) -> np.ndarray:
    envelope = np.zeros(length, dtype=np.float64)
    attack = max(1, min(attack, length))
    envelope[:attack] = np.linspace(0.0, 1.0, attack, endpoint=False)
    decay_end = min(length, attack + decay)
    if decay_end > attack:
        envelope[attack:decay_end] = np.linspace(1.0, sustain_level, decay_end - attack, endpoint=False)
    release_start = max(decay_end, length - release)
    if release_start < length:
        envelope[decay_end:release_start] = sustain_level
        envelope[release_start:] = np.linspace(sustain_level, 0.0, length - release_start)
    else:
        envelope[decay_end:] = sustain_level
    return envelope


def _one_pole_lowpass(signal: np.ndarray, cutoff_hz: float, sample_rate: int) -> np.ndarray:
    cutoff_hz = max(20.0, min(cutoff_hz, sample_rate / 2.0 - 1.0))
    alpha = 1.0 - np.exp(-2.0 * np.pi * cutoff_hz / sample_rate)
    out = np.empty_like(signal)
    state = 0.0
    for index, value in enumerate(signal):
        state += alpha * (value - state)
        out[index] = state
    return out


def _midi_to_hz(pitch: int) -> float:
    return 440.0 * (2.0 ** ((pitch - 69) / 12.0))


def render_wav(score: Score, path: str | Path, *, sample_rate: int = SAMPLE_RATE) -> Path:
    """Render ``score`` deterministically to a stereo 16-bit PCM WAV file."""

    validate_score(score)
    total_ticks = score.total_ticks()
    seconds_per_tick = 60.0 / (score.tempo_bpm * score.ppq)
    total_seconds = min(MAX_RENDER_SECONDS, total_ticks * seconds_per_tick + 0.5)
    total_samples = max(1, int(total_seconds * sample_rate))
    stereo = np.zeros((total_samples, 2), dtype=np.float64)

    for note in score.notes:
        control = score.synth.get(note.voice)
        oscillator = control.oscillator if control else "sine"
        gain = control.gain if control else 0.8
        pan = control.pan if control else 0.0
        attack_ms = control.attack_ms if control else 10.0
        decay_ms = control.decay_ms if control else 30.0
        sustain_level = control.sustain_level if control else 0.7
        release_ms = control.release_ms if control else 80.0
        cutoff_hz = control.filter_cutoff_hz if control else sample_rate / 4.0

        start_sample = int(note.start_tick * seconds_per_tick * sample_rate)
        length = int(note.duration_ticks * seconds_per_tick * sample_rate)
        if length <= 0 or start_sample >= total_samples:
            continue
        length = min(length, total_samples - start_sample)
        time = np.arange(length, dtype=np.float64) / sample_rate
        frequency = _midi_to_hz(note.pitch)
        waveform = _oscillator(oscillator, time * frequency)
        waveform = _one_pole_lowpass(waveform, cutoff_hz, sample_rate)
        envelope = _envelope(
            length,
            int(attack_ms * sample_rate / 1000.0),
            int(decay_ms * sample_rate / 1000.0),
            sustain_level,
            int(release_ms * sample_rate / 1000.0),
        )
        velocity_gain = note.velocity / 127.0
        voiced = waveform * envelope * gain * velocity_gain
        left = voiced * (1.0 - max(0.0, pan))
        right = voiced * (1.0 + min(0.0, pan))
        stereo[start_sample : start_sample + length, 0] += left
        stereo[start_sample : start_sample + length, 1] += right

    peak = np.abs(stereo).max()
    if peak > 0:
        stereo = stereo * (PEAK_AMPLITUDE / peak)
    if not np.isfinite(stereo).all():
        raise ValueError("rendered audio contains non-finite samples")
    quantized = np.clip(stereo * 32767.0, -32768, 32767).astype(np.int16)

    resolved = Path(path).expanduser().resolve()
    resolved.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(resolved), "wb") as handle:
        handle.setnchannels(2)
        handle.setsampwidth(2)
        handle.setframerate(sample_rate)
        handle.writeframes(quantized.tobytes())
    return resolved


__all__ = ["MAX_RENDER_SECONDS", "PEAK_AMPLITUDE", "SAMPLE_RATE", "render_wav"]

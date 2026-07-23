"""A small deterministic synthetic byte corpus for the H1 ablation pilot.

The retained enwik8/text8 protocol (``PROTOCOL.md``, ``CONFIRMATION.md``) is
frozen and is not touched by this study: it does not download or reuse either
corpus.  H1's smallest disproving experiment (``PAPER.md`` section 8.2) only
needs a byte sequence with genuine multi-scale structure so a dyadic mixer has
something to exploit; it does not need 100MB of natural-language text.  This
generator produces that sequence deterministically from an integer seed by
summing sinusoids at dyadic periods plus additive noise, so every pilot run is
exactly reproducible and its provenance is the seed and this file, not a
downloaded artifact.
"""

from __future__ import annotations

import hashlib
from pathlib import Path

import numpy as np

DEFAULT_PERIODS = (2, 4, 8, 16, 32, 64, 128, 256)


def synthetic_bytes(
    total_bytes: int,
    seed: int,
    *,
    periods: tuple[int, ...] = DEFAULT_PERIODS,
    noise_scale: float = 0.15,
) -> bytes:
    if total_bytes < 3:
        raise ValueError("total_bytes must be at least 3")
    random = np.random.default_rng(seed)
    index = np.arange(total_bytes, dtype=np.float64)
    signal = np.zeros(total_bytes, dtype=np.float64)
    for period in periods:
        phase = random.uniform(0.0, 2.0 * np.pi)
        amplitude = random.uniform(0.5, 1.0) / len(periods)
        signal += amplitude * np.sin(2.0 * np.pi * index / period + phase)
    noise = random.normal(0.0, noise_scale, size=total_bytes)
    values = signal + noise
    values = (values - values.min()) / max(values.max() - values.min(), 1e-9)
    return np.clip(np.round(values * 255.0), 0, 255).astype(np.uint8).tobytes()


def write_synthetic_corpus(path: str | Path, total_bytes: int, seed: int) -> Path:
    """Write (or reuse, if already present and matching) a synthetic corpus."""

    resolved = Path(path).expanduser().resolve()
    payload = synthetic_bytes(total_bytes, seed)
    if resolved.exists() and resolved.read_bytes() == payload:
        return resolved
    resolved.parent.mkdir(parents=True, exist_ok=True)
    resolved.write_bytes(payload)
    return resolved


def sha256_of(path: str | Path) -> str:
    return hashlib.sha256(Path(path).expanduser().resolve().read_bytes()).hexdigest()


__all__ = ["DEFAULT_PERIODS", "sha256_of", "synthetic_bytes", "write_synthetic_corpus"]

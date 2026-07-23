"""Immutable raw-byte corpora and shared benchmark batch schedules."""

from __future__ import annotations

import hashlib
from dataclasses import asdict, dataclass
from pathlib import Path

import numpy as np


@dataclass(frozen=True)
class CorpusMetadata:
    path: str
    sha256: str
    bytes: int
    train_bytes: int
    validation_bytes: int
    test_bytes: int
    split: tuple[float, float, float]

    def to_dict(self) -> dict:
        return asdict(self)


class ByteCorpus:
    """A byte corpus with an immutable contiguous train/validation/test split."""

    def __init__(
        self,
        path: str | Path,
        split: tuple[float, float, float] = (0.90, 0.05, 0.05),
    ) -> None:
        if len(split) != 3 or any(value <= 0 for value in split):
            raise ValueError("split must contain three positive fractions")
        if not np.isclose(sum(split), 1.0):
            raise ValueError("split fractions must sum to one")
        self.path = Path(path).expanduser().resolve()
        payload = self.path.read_bytes()
        if len(payload) < 3:
            raise ValueError("corpus is too small")
        values = np.frombuffer(payload, dtype=np.uint8).astype(np.int32)
        train_end = int(len(values) * split[0])
        validation_end = train_end + int(len(values) * split[1])
        self.train = values[:train_end]
        self.validation = values[train_end:validation_end]
        self.test = values[validation_end:]
        self.metadata = CorpusMetadata(
            path=str(self.path),
            sha256=hashlib.sha256(payload).hexdigest(),
            bytes=len(payload),
            train_bytes=len(self.train),
            validation_bytes=len(self.validation),
            test_bytes=len(self.test),
            split=split,
        )

    def data(self, split: str) -> np.ndarray:
        try:
            return {
                "train": self.train,
                "validation": self.validation,
                "test": self.test,
            }[split]
        except KeyError as exc:
            raise ValueError("split must be train, validation, or test") from exc

    def windows(
        self, split: str, starts: np.ndarray, context_length: int
    ) -> tuple[np.ndarray, np.ndarray]:
        data = self.data(split)
        starts = np.asarray(starts, dtype=np.int64)
        if starts.ndim != 1 or not len(starts):
            raise ValueError("starts must be a non-empty one-dimensional array")
        if starts.min() < 0 or starts.max() + context_length >= len(data):
            raise ValueError(f"window exceeds {split} split")
        offsets = np.arange(context_length, dtype=np.int64)
        x = data[starts[:, None] + offsets]
        y = data[starts[:, None] + offsets + 1]
        return np.ascontiguousarray(x), np.ascontiguousarray(y)

    def evaluation_batches(
        self,
        split: str,
        *,
        context_length: int,
        windows: int,
        batch_size: int,
    ) -> list[tuple[np.ndarray, np.ndarray]]:
        if split == "train":
            raise ValueError("evaluation must use a held-out split")
        maximum = len(self.data(split)) - context_length - 1
        if maximum < 0:
            raise ValueError(f"{split} split is shorter than the context")
        count = min(windows, maximum + 1)
        starts = np.linspace(0, maximum, num=count, dtype=np.int64)
        return [
            self.windows(split, starts[index : index + batch_size], context_length)
            for index in range(0, len(starts), batch_size)
        ]


class BatchSchedule:
    """Precomputed training starts shared byte-for-byte by paired models."""

    def __init__(
        self,
        corpus: ByteCorpus,
        *,
        steps: int,
        batch_size: int,
        context_length: int,
        seed: int,
    ) -> None:
        maximum = len(corpus.train) - context_length
        if steps < 1 or batch_size < 1 or maximum < 1:
            raise ValueError("invalid schedule dimensions")
        random = np.random.default_rng(seed)
        self.starts = random.integers(
            0, maximum, size=(steps, batch_size), dtype=np.int64
        )
        self.sha256 = hashlib.sha256(self.starts.tobytes()).hexdigest()

    def batch(
        self, corpus: ByteCorpus, step: int, context_length: int
    ) -> tuple[np.ndarray, np.ndarray]:
        return corpus.windows("train", self.starts[step], context_length)

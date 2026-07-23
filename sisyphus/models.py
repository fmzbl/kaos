"""Sisyphus and a parameter-matched modern Transformer control.

The model is based on the *semantics* of Rebis, not on magical punctuation:

``quote``
    Retain the current representation unchanged.
``route`` (``->``)
    Accept evidence from an earlier causal location.
``group`` (``(...)``)
    Compose local and routed evidence.
``square`` (``[M]``)
    Let a learned mediator reconcile direct and detour candidates.

One parameter-shared cell softly chooses among these four operations.  It is
reused over power-of-two causal offsets, giving every position a logarithmic
route to its complete prefix, and then reused over refinement rounds.  The
unrolled computation grows with context and rounds while stored parameters do
not.  Later rounds receive deep supervision and a monotonicity penalty during
training; this is bounded internal revision, not a claim that the model can
safely rewrite its own objective or promote its own weights.
"""

from __future__ import annotations

import math
from dataclasses import asdict, dataclass

import numpy as np
from tinygrad import Tensor, nn

from .layers import Linear, RMSNorm, SwiGLU, optimizer_parameters, parameter_count


def _sinusoidal_positions(length: int, width: int) -> np.ndarray:
    """Fixed positions keep parameter storage independent of context length."""

    position = np.arange(length, dtype=np.float32)[:, None]
    pairs = (width + 1) // 2
    inverse = 1.0 / (10000.0 ** (np.arange(pairs, dtype=np.float32) / pairs))
    angles = position * inverse[None, :]
    values = np.zeros((length, width), dtype=np.float32)
    values[:, 0::2] = np.sin(angles[:, : values[:, 0::2].shape[1]])
    values[:, 1::2] = np.cos(angles[:, : values[:, 1::2].shape[1]])
    # Match the scale of the token and former learned-position initialization;
    # unit-amplitude sinusoids dominate a 0.02-initialized tiny model.
    return values * 0.02


@dataclass(frozen=True)
class ModelSpec:
    name: str
    family: str
    vocab_size: int
    context_length: int
    width: int
    layers: int
    ffn_hidden: int
    heads: int | None = None
    rounds: int | None = None

    def to_dict(self) -> dict:
        return asdict(self)


class RebisCell:
    """A parameter-shared quote/route/group/square causal cell."""

    OPERATORS = ("quote", "route", "group", "square")

    def __init__(self, width: int, levels: int, rounds: int) -> None:
        self.width = width
        self.levels = levels
        self.rounds = rounds
        self.norm = RMSNorm(width)
        self.local = Linear(width, width, bias=False)
        self.route = Linear(width, width, bias=False)
        self.mediator = Linear(2 * width, width, bias=False)
        self.operator = Linear(2 * width, len(self.OPERATORS), bias=True)
        self.output = Linear(width, width, bias=False)
        self.level_code = Tensor.randn(levels, width) * 0.02
        self.round_code = Tensor.randn(rounds, width) * 0.02

    @staticmethod
    def _causal_shift(value: Tensor, offset: int) -> Tensor:
        """Shift right by ``offset``; the introduced prefix is exactly zero."""

        batch, length, width = value.shape
        if offset >= length:
            return Tensor.zeros(batch, length, width, device=value.device)
        prefix = Tensor.zeros(batch, offset, width, device=value.device)
        return prefix.cat(value[:, : length - offset], dim=1)

    def __call__(self, state: Tensor, level: int, round_index: int) -> Tensor:
        offset = 1 << level
        normed = self.norm(state)
        routed_state = self._causal_shift(normed, offset)
        code = (
            self.level_code[level] + self.round_code[round_index]
        ).reshape(1, 1, self.width)
        local = self.local(normed) + code
        routed = self.route(routed_state)

        # The direct path groups the two currents.  The detour passes through a
        # typed persistent square (the mediator map); no route bypasses the
        # four-way operator choice that records how the update was formed.
        grouped = 0.5 * (local + routed)
        mediated = self.mediator(local.cat(routed, dim=-1)).tanh()
        weights = self.operator(local.cat(routed, dim=-1)).softmax(axis=-1)
        choices = Tensor.stack(normed, routed, grouped, mediated, dim=2)
        mixed = (choices * weights.unsqueeze(-1)).sum(axis=2)
        return self.output(mixed)


class SisyphusBlock:
    """Logarithmic causal routing followed by bounded recursive revision."""

    def __init__(
        self,
        width: int,
        context_length: int,
        rounds: int,
        ffn_hidden: int,
    ) -> None:
        if context_length < 2 or context_length & (context_length - 1):
            raise ValueError("context length must be a power of two >= 2")
        self.width = width
        self.context_length = context_length
        self.levels = int(math.log2(context_length))
        self.rounds = rounds
        self.cell = RebisCell(width, self.levels, rounds)
        self.ffn_norm = RMSNorm(width)
        self.ffn = SwiGLU(width, ffn_hidden)

    def __call__(self, state: Tensor) -> tuple[Tensor, list[Tensor]]:
        length = state.shape[1]
        if length < 2 or length & (length - 1):
            raise ValueError("sequence length must be a power of two >= 2")
        if length > self.context_length:
            raise ValueError("sequence exceeds configured context length")
        active_levels = int(math.log2(length))
        residual_scale = 1.0 / math.sqrt(max(1, self.rounds * active_levels))
        rounds: list[Tensor] = []
        for round_index in range(self.rounds):
            for level in range(active_levels):
                state = state + residual_scale * self.cell(state, level, round_index)
            state = state + residual_scale * self.ffn(self.ffn_norm(state))
            rounds.append(state)
        return state, rounds


class SisyphusLM:
    """Decoder-only model with recursive, parameter-shared Rebis operations."""

    def __init__(
        self,
        *,
        vocab_size: int = 256,
        context_length: int = 128,
        width: int = 41,
        layers: int = 1,
        rounds: int = 2,
        ffn_hidden: int = 82,
        intermediate_weight: float = 0.15,
        monotonic_weight: float = 0.05,
    ) -> None:
        self.vocab_size = vocab_size
        self.context_length = context_length
        self.width = width
        self.layers = layers
        self.rounds = rounds
        self.intermediate_weight = intermediate_weight
        self.monotonic_weight = monotonic_weight
        self.token_embed = nn.Embedding(vocab_size, width)
        # Stored as NumPy rather than a Tensor so it is an immutable buffer,
        # excluded from both the optimizer and physical parameter count.
        self._position = _sinusoidal_positions(context_length, width)
        self.blocks = [
            SisyphusBlock(width, context_length, rounds, ffn_hidden)
            for _ in range(layers)
        ]
        self.out_norm = RMSNorm(width)
        self.head = Linear(width, vocab_size, bias=False)
        self.head.weight = self.token_embed.weight

    def hidden_rounds(self, tokens: Tensor) -> list[Tensor]:
        length = tokens.shape[1]
        if length > self.context_length:
            raise ValueError("sequence exceeds configured context length")
        position = Tensor(self._position[:length], device=tokens.device)
        state = self.token_embed(tokens) + position
        all_rounds: list[Tensor] = []
        for block in self.blocks:
            state, block_rounds = block(state)
            all_rounds.extend(block_rounds)
        return all_rounds

    def logits_by_round(self, tokens: Tensor) -> list[Tensor]:
        return [self.head(self.out_norm(state)) for state in self.hidden_rounds(tokens)]

    def __call__(self, tokens: Tensor) -> Tensor:
        return self.logits_by_round(tokens)[-1]

    def loss_terms(self, tokens: Tensor, targets: Tensor) -> tuple[Tensor, Tensor, Tensor]:
        logits = self.logits_by_round(tokens)
        losses = [
            value.reshape(-1, self.vocab_size).sparse_categorical_crossentropy(
                targets.reshape(-1), reduction="none"
            )
            for value in logits
        ]
        language = losses[-1].mean()
        if len(losses) == 1:
            return language, Tensor.zeros((), device=language.device), Tensor.zeros(
                (), device=language.device
            )
        intermediate = sum((loss.mean() for loss in losses[:-1]), Tensor.zeros(())).div(
            len(losses) - 1
        )
        monotonic = sum(
            ((after - before).relu().mean() for before, after in zip(losses, losses[1:])),
            Tensor.zeros(),
        ).div(len(losses) - 1)
        return language, intermediate, monotonic

    def language_loss(self, tokens: Tensor, targets: Tensor) -> Tensor:
        return self.loss_terms(tokens, targets)[0]

    def loss(self, tokens: Tensor, targets: Tensor) -> Tensor:
        language, intermediate, monotonic = self.loss_terms(tokens, targets)
        return (
            language
            + self.intermediate_weight * intermediate
            + self.monotonic_weight * monotonic
        )

    def optimizer_parameters(self) -> list[Tensor]:
        return optimizer_parameters(self)


class RotaryAttention:
    def __init__(self, width: int, heads: int, context_length: int) -> None:
        if width % heads:
            raise ValueError("width must be divisible by heads")
        self.width = width
        self.heads = heads
        self.head_dim = width // heads
        if self.head_dim % 2:
            raise ValueError("RoPE head dimension must be even")
        self.qkv = Linear(width, 3 * width, bias=False)
        self.output = Linear(width, width, bias=False)
        half = self.head_dim // 2
        inverse = 1.0 / (10000.0 ** (np.arange(half, dtype=np.float32) / half))
        angles = np.arange(context_length, dtype=np.float32)[:, None] * inverse[None, :]
        self._cos = np.cos(angles).astype(np.float32)
        self._sin = np.sin(angles).astype(np.float32)
        self._mask = np.triu(
            np.full((context_length, context_length), -np.inf, dtype=np.float32), 1
        )

    def _rope(self, value: Tensor) -> Tensor:
        length = value.shape[-2]
        half = self.head_dim // 2
        first, second = value[..., :half], value[..., half:]
        shape = (1, 1, length, half)
        cosine = Tensor(self._cos[:length], device=value.device).reshape(*shape)
        sine = Tensor(self._sin[:length], device=value.device).reshape(*shape)
        return (first * cosine - second * sine).cat(first * sine + second * cosine, dim=-1)

    def __call__(self, value: Tensor) -> Tensor:
        batch, length, _ = value.shape
        query, key, content = self.qkv(value).chunk(3, dim=-1)
        shape = (batch, length, self.heads, self.head_dim)
        query = self._rope(query.reshape(*shape).transpose(1, 2))
        key = self._rope(key.reshape(*shape).transpose(1, 2))
        content = content.reshape(*shape).transpose(1, 2)
        mask = Tensor(self._mask[:length, :length], device=value.device).reshape(
            1, 1, length, length
        )
        weights = (
            (query @ key.transpose(-2, -1)) / math.sqrt(self.head_dim) + mask
        ).softmax(-1)
        mixed = (weights @ content).transpose(1, 2).reshape(batch, length, self.width)
        return self.output(mixed)


class TransformerBlock:
    def __init__(self, width: int, heads: int, hidden: int, context_length: int) -> None:
        self.attention_norm = RMSNorm(width)
        self.attention = RotaryAttention(width, heads, context_length)
        self.ffn_norm = RMSNorm(width)
        self.ffn = SwiGLU(width, hidden)

    def __call__(self, value: Tensor) -> Tensor:
        value = value + self.attention(self.attention_norm(value))
        return value + self.ffn(self.ffn_norm(value))


class ModernTransformerLM:
    """Llama-style causal Transformer control."""

    def __init__(
        self,
        *,
        vocab_size: int = 256,
        context_length: int = 128,
        width: int = 32,
        layers: int = 2,
        heads: int = 4,
        ffn_hidden: int = 69,
    ) -> None:
        self.vocab_size = vocab_size
        self.context_length = context_length
        self.token_embed = nn.Embedding(vocab_size, width)
        self.blocks = [
            TransformerBlock(width, heads, ffn_hidden, context_length)
            for _ in range(layers)
        ]
        self.out_norm = RMSNorm(width)
        self.head = Linear(width, vocab_size, bias=False)
        self.head.weight = self.token_embed.weight

    def __call__(self, tokens: Tensor) -> Tensor:
        if tokens.shape[1] > self.context_length:
            raise ValueError("sequence exceeds configured context length")
        state = self.token_embed(tokens)
        for block in self.blocks:
            state = block(state)
        return self.head(self.out_norm(state))

    def language_loss(self, tokens: Tensor, targets: Tensor) -> Tensor:
        logits = self(tokens)
        return logits.reshape(-1, self.vocab_size).sparse_categorical_crossentropy(
            targets.reshape(-1)
        )

    def loss(self, tokens: Tensor, targets: Tensor) -> Tensor:
        return self.language_loss(tokens, targets)

    def optimizer_parameters(self) -> list[Tensor]:
        return optimizer_parameters(self)


def build_model(
    name: str,
    *,
    context_length: int = 128,
    vocab_size: int = 256,
) -> tuple[object, ModelSpec, int]:
    """Construct the pre-specified matched models used by the benchmark."""

    if name == "sisyphus":
        spec = ModelSpec(
            name="sisyphus",
            family="sisyphus",
            vocab_size=vocab_size,
            context_length=context_length,
            width=41,
            layers=1,
            ffn_hidden=82,
            rounds=2,
        )
        model = SisyphusLM(
            vocab_size=vocab_size,
            context_length=context_length,
            width=spec.width,
            layers=spec.layers,
            rounds=spec.rounds or 1,
            ffn_hidden=spec.ffn_hidden,
        )
    elif name == "transformer":
        spec = ModelSpec(
            name="transformer",
            family="transformer",
            vocab_size=vocab_size,
            context_length=context_length,
            width=32,
            layers=2,
            ffn_hidden=69,
            heads=4,
        )
        model = ModernTransformerLM(
            vocab_size=vocab_size,
            context_length=context_length,
            width=spec.width,
            layers=spec.layers,
            heads=spec.heads or 1,
            ffn_hidden=spec.ffn_hidden,
        )
    else:
        raise ValueError(f"unknown model {name!r}")
    return model, spec, parameter_count(model)


__all__ = [
    "ModelSpec",
    "ModernTransformerLM",
    "SisyphusBlock",
    "SisyphusLM",
    "build_model",
    "parameter_count",
]

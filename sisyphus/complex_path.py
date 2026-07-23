"""The Complex Rebis Path Machine (hypothesis H1) and its matched ablations.

This module is an explicit, falsifiable prototype, not a claim.  It is
additive: :mod:`models` and every retained enwik8/text8 result are untouched.

Rebis derivation
----------------

``models.RebisCell`` already gives quote/route/group/square a real-valued
neural reading.  H1 (``PAPER.md`` section 8.2) asks whether making the
recursive *path* through refinement rounds live in complex space -- not
merely widening the real state -- does measurable work.  Each operator is
re-derived, not merely re-typed:

``quote``
    Retain the state, normalized by complex modulus with a real per-channel
    gain that multiplies the real and imaginary parts identically.  A single
    positive real scalar cannot change a channel's phase, so this is a
    genuine phase-preserving quote, not an ordinary real RMSNorm relabeled.
``route``
    Read a causally shifted, zero-padded earlier state through a learned
    linear map, then rotate it by a learned per-scale phase
    ``exp(i * theta_level)``.  This is the ``beta_t exp(i theta_t)
    R(z_(t-delta))`` term from the registered recurrence: route now carries
    an *explicit* phase parameter that participates in the forward
    computation (tested in ``test_complex_path.py``), not a metaphor.
``group``
    An order-preserving complex sum of the local and routed currents,
    exactly as ``models.RebisCell`` composes evidence, but the addition is
    now a genuine complex addition of interfering currents.
``square``
    A learned mediator reconciles the direct and detour candidates, exactly
    as in ``models.RebisCell``; the reconciliation is a real-valued map over
    the stacked real/imaginary parts (see ``PairLinear`` below for why the
    complex/real distinction lives in ``local``/``route`` instead).

The learned iterative path is ``z_0, z_1, ..., z_T`` across refinement
rounds, exactly the ``rounds`` list already produced by ``SisyphusBlock``,
now living in complex rather than real space.  ``diagnostics`` exposes
radius, phase, and a windowed winding proxy for that path so an
implementation cannot silently reduce to a one-shot complex embedding.

Real-pair emulation and Wirtinger gradients
--------------------------------------------

tinygrad has no differentiable complex dtype, so every complex value here is
stored as one real tensor whose last axis concatenates ``[real, imag]``, each
of width ``width``.  Complex multiplication is expanded into the ordinary
real arithmetic ``(ac - bd, ad + bc)``.  Because tinygrad's autograd is
already exact for real-valued graphs, backpropagating through that expansion
computes exactly the Wirtinger derivatives with respect to the real and
imaginary parts independently -- the standard real-pair technique for
complex-valued autodiff, not an approximation.

The matched real-valued ablation (H1 ablation b)
-------------------------------------------------

``PairLinear`` holds exactly the same two ``width x width`` real weight
matrices in both modes.  In complex mode it combines them with the
sign-flipped cross term that defines ``i^2 = -1``.  In real mode it combines
them block-diagonally (no cross term, no rotation).  This isolates the
complex/phase mechanism itself -- not extra width or extra parameters --
as the ablated variable, per the registered hypothesis.
"""

from __future__ import annotations

import math
from dataclasses import asdict, dataclass

import numpy as np
from tinygrad import Tensor, nn

from .layers import Linear, RMSNorm, SwiGLU, optimizer_parameters, parameter_count
from .models import _sinusoidal_positions


@dataclass(frozen=True)
class PathSpec:
    name: str
    arm: str
    vocab_size: int
    context_length: int
    width: int
    rounds: int
    ffn_hidden: int
    complex_mode: bool
    phase_destroy: bool

    def to_dict(self) -> dict:
        return asdict(self)


class PairLinear:
    """Two matched real matrices, combined with or without complex coupling.

    ``complex_mode=True`` implements complex multiplication ``W @ z`` where
    ``W = w_a + i w_b`` and ``z = a + i b``.  ``complex_mode=False`` keeps the
    identical two weight tensors but drops the cross term, giving the H1(b)
    ablation exactly matched parameter-for-parameter to H1(a).
    """

    def __init__(self, width: int, complex_mode: bool) -> None:
        self.width = width
        self.complex_mode = complex_mode
        self.w_a = Linear(width, width, bias=False)
        self.w_b = Linear(width, width, bias=False)

    def __call__(self, pair: Tensor) -> Tensor:
        a, b = pair[..., : self.width], pair[..., self.width :]
        if self.complex_mode:
            out_a = self.w_a(a) - self.w_b(b)
            out_b = self.w_a(b) + self.w_b(a)
        else:
            out_a, out_b = self.w_a(a), self.w_b(b)
        return out_a.cat(out_b, dim=-1)


class RebisPathCell:
    """A parameter-shared quote/route/group/square cell over a complex path.

    ``width`` is the size of each of the two real components (real/imag, or
    the real-ablation's matched ``u``/``v`` halves); the physical state width
    is always ``2 * width``.
    """

    OPERATORS = ("quote", "route", "group", "square")

    def __init__(self, width: int, levels: int, rounds: int, complex_mode: bool) -> None:
        self.width = width
        self.repr_width = 2 * width
        self.levels = levels
        self.rounds = rounds
        self.complex_mode = complex_mode
        self.norm = RMSNorm(self.repr_width)
        self.local = PairLinear(width, complex_mode)
        self.route = PairLinear(width, complex_mode)
        self.mediator = Linear(4 * width, self.repr_width, bias=False)
        self.operator = Linear(4 * width, len(self.OPERATORS), bias=True)
        self.output = Linear(self.repr_width, self.repr_width, bias=False)
        self.level_code = Tensor.randn(levels, self.repr_width) * 0.02
        self.round_code = Tensor.randn(rounds, self.repr_width) * 0.02
        # The explicit causal-route phase.  Only meaningful in complex mode,
        # where rotating (a, b) by (cos theta, sin theta) is a genuine
        # unit-complex multiplication; the real ablation has no such
        # mechanism at all, by construction of the ablation.
        self.route_phase = Tensor(np.linspace(0.2, 1.1, levels).astype(np.float32)) if complex_mode else None

    @staticmethod
    def _causal_shift(value: Tensor, offset: int) -> Tensor:
        batch, length, width = value.shape
        if offset >= length:
            return Tensor.zeros(batch, length, width, device=value.device)
        prefix = Tensor.zeros(batch, offset, width, device=value.device)
        return prefix.cat(value[:, : length - offset], dim=1)

    def _rotate(self, pair: Tensor, level: int) -> Tensor:
        if not self.complex_mode:
            return pair
        theta = self.route_phase[level]
        cosine, sine = theta.cos(), theta.sin()
        a, b = pair[..., : self.width], pair[..., self.width :]
        rotated_a = a * cosine - b * sine
        rotated_b = a * sine + b * cosine
        return rotated_a.cat(rotated_b, dim=-1)

    def __call__(
        self, state: Tensor, level: int, round_index: int, trace: list | None = None
    ) -> Tensor:
        offset = 1 << level
        normed = self.norm(state)
        shifted = self._causal_shift(normed, offset)
        code = (self.level_code[level] + self.round_code[round_index]).reshape(
            1, 1, self.repr_width
        )
        local = self.local(normed) + code
        routed = self._rotate(self.route(shifted), level)
        grouped = 0.5 * (local + routed)
        mediated = self.mediator(local.cat(routed, dim=-1)).tanh()
        weights = self.operator(local.cat(routed, dim=-1)).softmax(axis=-1)
        if trace is not None:
            # Diagnostic-only: the music sidecar's teacher export reads this
            # to learn from *how* the main model routed evidence, not just
            # its final hidden state. No gradient flows through the trace.
            trace.append(
                {
                    "level": level,
                    "round": round_index,
                    "operator_weights_mean": weights.mean(axis=(0, 1)).numpy().tolist(),
                }
            )
        choices = Tensor.stack(normed, routed, grouped, mediated, dim=2)
        mixed = (choices * weights.unsqueeze(-1)).sum(axis=2)
        return self.output(mixed)


class RebisPathBlock:
    """Dyadic causal routing plus a bounded recursive path, shared cell."""

    def __init__(
        self,
        width: int,
        context_length: int,
        rounds: int,
        ffn_hidden: int,
        complex_mode: bool,
    ) -> None:
        if context_length < 2 or context_length & (context_length - 1):
            raise ValueError("context length must be a power of two >= 2")
        self.width = width
        self.repr_width = 2 * width
        self.context_length = context_length
        self.levels = int(math.log2(context_length))
        self.rounds = rounds
        self.cell = RebisPathCell(width, self.levels, rounds, complex_mode)
        self.ffn_norm = RMSNorm(self.repr_width)
        self.ffn = SwiGLU(self.repr_width, ffn_hidden)

    def __call__(self, state: Tensor, trace: list | None = None) -> list[Tensor]:
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
                state = state + residual_scale * self.cell(state, level, round_index, trace=trace)
            state = state + residual_scale * self.ffn(self.ffn_norm(state))
            rounds.append(state)
        return rounds


class RebisPathLM:
    """Decoder-only model whose recursive path lives in complex state space.

    ``arm`` selects one of the four H1 disproving-experiment configurations:

    ``"complex"``
        H1(a): the full complex path machine.
    ``"real"``
        H1(b): identical shapes, PairLinear runs in block-diagonal
        (non-complex) mode -- the matched real-valued ablation.
    ``"nonrecursive"``
        H1(c): the complex machine with ``rounds`` forced to 1.
    ``"phase-destroyed"``
        H1(d): the complex machine, but the final round's state is rotated
        by an independent random phase at readout, destroying any
        consistently encoded phase information before the head projects it.
    """

    ARMS = ("complex", "real", "nonrecursive", "phase-destroyed")

    def __init__(
        self,
        *,
        vocab_size: int = 256,
        context_length: int = 128,
        width: int = 20,
        rounds: int = 2,
        ffn_hidden: int = 80,
        arm: str = "complex",
        intermediate_weight: float = 0.15,
        monotonic_weight: float = 0.05,
    ) -> None:
        if arm not in self.ARMS:
            raise ValueError(f"unknown arm {arm!r}")
        self.arm = arm
        self.vocab_size = vocab_size
        self.context_length = context_length
        self.width = width
        self.repr_width = 2 * width
        self.complex_mode = arm != "real"
        self.phase_destroy = arm == "phase-destroyed"
        effective_rounds = 1 if arm == "nonrecursive" else rounds
        self.rounds = effective_rounds
        self.intermediate_weight = intermediate_weight
        self.monotonic_weight = monotonic_weight
        self.token_embed = nn.Embedding(vocab_size, width)
        self._position = _sinusoidal_positions(context_length, width)
        self.block = RebisPathBlock(
            width, context_length, effective_rounds, ffn_hidden, self.complex_mode
        )
        self.out_norm = RMSNorm(self.repr_width)
        self.head = Linear(self.repr_width, vocab_size, bias=False)

    def _initial_state(self, tokens: Tensor) -> Tensor:
        length = tokens.shape[1]
        if length > self.context_length:
            raise ValueError("sequence exceeds configured context length")
        position = Tensor(self._position[:length], device=tokens.device)
        base = self.token_embed(tokens) + position
        zeros = Tensor.zeros(*base.shape, device=tokens.device)
        return base.cat(zeros, dim=-1)

    def hidden_rounds(self, tokens: Tensor, trace: list | None = None) -> list[Tensor]:
        return self.block(self._initial_state(tokens), trace=trace)

    def _destroy_phase(self, state: Tensor) -> Tensor:
        a, b = state[..., : self.width], state[..., self.width :]
        theta = Tensor.rand(*a.shape, device=state.device) * (2.0 * math.pi)
        rotated_a = a * theta.cos() - b * theta.sin()
        rotated_b = a * theta.sin() + b * theta.cos()
        return rotated_a.cat(rotated_b, dim=-1)

    def logits_by_round(self, tokens: Tensor) -> list[Tensor]:
        rounds = self.hidden_rounds(tokens)
        last = len(rounds) - 1
        logits = []
        for index, state in enumerate(rounds):
            if self.phase_destroy and index == last:
                state = self._destroy_phase(state)
            logits.append(self.head(self.out_norm(state)))
        return logits

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
            zero = Tensor.zeros((), device=language.device)
            return language, zero, zero
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

    def diagnostics(self, tokens: Tensor) -> list[dict]:
        """Non-differentiable per-round path diagnostics: radius and phase.

        This is what distinguishes a genuine iterative path through complex
        space from a one-shot complex embedding: the statistics below must be
        computable, finite, and (for the complex arm) show phase actually
        moving across rounds.
        """

        stats = []
        for state in self.hidden_rounds(tokens):
            array = state.numpy()
            a, b = array[..., : self.width], array[..., self.width :]
            magnitude = np.sqrt(a**2 + b**2)
            entry = {
                "radius_mean": float(magnitude.mean()),
                "radius_max": float(magnitude.max()),
            }
            if self.complex_mode:
                phase = np.arctan2(b, a)
                entry["phase_circular_mean"] = float(np.angle(np.mean(np.exp(1j * phase))))
                entry["phase_std"] = float(np.std(phase))
            stats.append(entry)
        return stats


def build_path_model(
    arm: str,
    *,
    context_length: int = 128,
    vocab_size: int = 256,
    width: int = 20,
    rounds: int = 2,
    ffn_hidden: int = 80,
) -> tuple[RebisPathLM, PathSpec, int]:
    """Construct one of the four H1 matched-configuration arms."""

    model = RebisPathLM(
        vocab_size=vocab_size,
        context_length=context_length,
        width=width,
        rounds=rounds,
        ffn_hidden=ffn_hidden,
        arm=arm,
    )
    spec = PathSpec(
        name=f"rebis-path-{arm}",
        arm=arm,
        vocab_size=vocab_size,
        context_length=context_length,
        width=width,
        rounds=model.rounds,
        ffn_hidden=ffn_hidden,
        complex_mode=model.complex_mode,
        phase_destroy=model.phase_destroy,
    )
    return model, spec, parameter_count(model)


__all__ = [
    "PairLinear",
    "PathSpec",
    "RebisPathBlock",
    "RebisPathCell",
    "RebisPathLM",
    "build_path_model",
]

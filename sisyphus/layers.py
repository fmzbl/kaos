"""Small tinygrad layers shared by Sisyphus and its control.

The code is deliberately dependency-light so the experiment can run in the same
tinygrad environment as Thoth.  These definitions follow the conventional
Llama-style control already used by Thoth's benchmark, but live here so the new
study does not import mutable code from a sibling checkout.
"""

from __future__ import annotations

from tinygrad import Tensor, nn


class Linear(nn.Linear):
    """Named local seam for linear maps used by both benchmark arms."""


class RMSNorm:
    def __init__(self, width: int, eps: float = 1e-6) -> None:
        self.weight = Tensor.ones(width)
        self.eps = eps

    def __call__(self, value: Tensor) -> Tensor:
        return (
            value
            * (value.square().mean(axis=-1, keepdim=True) + self.eps).rsqrt()
            * self.weight
        )


class SwiGLU:
    def __init__(self, width: int, hidden: int) -> None:
        self.gate = Linear(width, hidden, bias=False)
        self.value = Linear(width, hidden, bias=False)
        self.output = Linear(hidden, width, bias=False)

    def __call__(self, value: Tensor) -> Tensor:
        return self.output(self.gate(value).silu() * self.value(value))


def parameter_count(model: object) -> int:
    """Count physical parameters once, including tied tensors."""

    unique = {id(value): value for value in nn.state.get_parameters(model)}
    return sum(value.numel() for value in unique.values())


def optimizer_parameters(model: object) -> list[Tensor]:
    """Return physical trainable tensors once, including tied embeddings."""

    return list({id(value): value for value in nn.state.get_parameters(model)}.values())

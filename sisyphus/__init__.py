"""First-class Sisyphus recursive language-model component for Kaos."""

from .models import ModernTransformerLM, SisyphusLM, build_model, parameter_count

__all__ = [
    "ModernTransformerLM",
    "SisyphusLM",
    "build_model",
    "parameter_count",
]

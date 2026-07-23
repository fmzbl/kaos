"""Select a tinygrad CPU compiler without requiring shell configuration."""

from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path


def ensure_compiler() -> str | None:
    """Use clang when available, otherwise Thoth's installed ziglang shim."""

    existing = os.environ.get("CC")
    if existing and existing.strip():
        return existing
    if shutil.which("clang"):
        return None
    # The package does not vendor a compiler binary. Its small executable shim
    # translates tinygrad's clang-style target and calls ziglang's bundled zig.
    shim = Path(__file__).resolve().with_name("zigcc")
    try:
        import ziglang  # noqa: F401
    except ImportError:
        return None
    if shim.is_file() and os.access(shim, os.X_OK):
        os.environ["CC"] = str(shim)
        os.environ["SISYPHUS_ZIGCC_PYTHON"] = sys.executable
        return str(shim)
    zig = shutil.which("zig")
    if zig:
        os.environ["CC"] = zig
        return zig
    return None


__all__ = ["ensure_compiler"]

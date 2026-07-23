"""Unified command surface for the root-level Sisyphus component."""

from __future__ import annotations

import importlib
import sys
from collections.abc import Sequence


COMMANDS = {
    "benchmark": ("sisyphus.benchmark_lm", "run the paired byte-LM benchmark"),
    "summarize": ("sisyphus.summarize", "recompute a paired quality summary"),
    "scaling": ("sisyphus.benchmark_scaling", "measure context-length scaling"),
    "summarize-scaling": (
        "sisyphus.summarize_scaling",
        "recompute a context-scaling summary",
    ),
    "improve": ("sisyphus.improve", "run validation-gated continual learning"),
    "complex-path-pilot": (
        "sisyphus.benchmark_complex_path",
        "run the H1 complex-path four-arm ablation pilot",
    ),
    "summarize-complex-path": (
        "sisyphus.summarize_complex_path",
        "apply the H1 falsification rule to a pilot's results",
    ),
    "music-sidecar": (
        "sisyphus.music_sidecar",
        "opt-in H2 music sidecar cycle (requires --enable)",
    ),
}


def usage() -> str:
    lines = [
        "usage: python -m sisyphus <command> [options]",
        "",
        "commands:",
    ]
    width = max(len(name) for name in COMMANDS)
    lines.extend(
        f"  {name:<{width}}  {description}"
        for name, (_, description) in COMMANDS.items()
    )
    lines.extend(("", "Use `python -m sisyphus <command> --help` for details."))
    return "\n".join(lines)


def main(argv: Sequence[str] | None = None) -> None:
    arguments = list(sys.argv[1:] if argv is None else argv)
    if not arguments or arguments[0] in {"-h", "--help"}:
        print(usage())
        return
    command, *forwarded = arguments
    try:
        module_name, _ = COMMANDS[command]
    except KeyError as exc:
        raise SystemExit(f"unknown Sisyphus command {command!r}\n\n{usage()}") from exc
    module = importlib.import_module(module_name)
    previous = sys.argv
    sys.argv = [f"python -m sisyphus {command}", *forwarded]
    try:
        module.main()
    finally:
        sys.argv = previous


if __name__ == "__main__":
    main()

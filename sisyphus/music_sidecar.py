"""CLI for the opt-in music sidecar (hypothesis H2).

Disabled by default: every subcommand requires ``--enable`` so a bare
invocation, a typo, or an automated sweep cannot silently start training or
writing artifacts. There is no daemon, no network service, and no microphone
capture anywhere in this module -- it is a sequence of bounded, offline batch
steps, each of which writes its result and exits.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
from typing import Any

import numpy as np
from tinygrad import Tensor
from tinygrad.nn.optim import Adam

from .compiler import ensure_compiler
from .complex_path import build_path_model
from .data import ByteCorpus
from .music.adapter import apply_gated_music_feedback
from .music.feedback import compute_feedback_packet, decode_score
from .music.midi import score_to_midi_bytes
from .music.sidecar_model import SidecarModel, loss_against_targets
from .music.synth import render_wav
from .music.target import teacher_to_score_and_targets
from .music.teacher import export_teacher, write_teacher_export
from .synthetic_data import write_synthetic_corpus


def _write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    os.replace(temporary, path)


def _config_sha256(config: dict) -> str:
    return hashlib.sha256(json.dumps(config, sort_keys=True).encode()).hexdigest()


def _train_sidecar(feature_vector: np.ndarray, targets, *, steps: int, levels: int, rounds: int, seed: int) -> tuple[SidecarModel, dict]:
    Tensor.manual_seed(seed)
    model = SidecarModel(feature_width=len(feature_vector), max_steps=max(64, len(targets.pitch_bucket)), max_rounds=max(8, rounds))
    optimizer = Adam(model.optimizer_parameters(), lr=5e-3)
    feature_tensor = Tensor(feature_vector[None, :])
    steps_count = len(targets.pitch_bucket)
    losses = []
    for _ in range(steps):
        with Tensor.train():
            optimizer.zero_grad()
            predictions = model.predict(feature_tensor, steps_count, levels, rounds)
            loss = loss_against_targets(predictions, targets, steps_count, rounds)
            loss.backward()
            optimizer.step()
            losses.append(float(loss.item()))
    return model, {"first_loss": losses[0], "final_loss": losses[-1], "steps": steps}


def run_cycle(args: argparse.Namespace) -> dict[str, Any]:
    ensure_compiler()
    args.artifact_dir.mkdir(parents=True, exist_ok=True)

    corpus_path = write_synthetic_corpus(args.corpus_path, args.corpus_bytes, args.corpus_seed)
    corpus = ByteCorpus(corpus_path)

    Tensor.manual_seed(args.main_model_seed)
    main_model, spec, main_params = build_path_model(
        args.main_arm, context_length=args.context_length, width=args.main_width, rounds=args.main_rounds
    )
    main_config_sha = _config_sha256({"spec": spec.to_dict(), "seed": args.main_model_seed})

    fixed_tokens = corpus.windows("validation", np.array([0]), args.context_length)[0]
    export = export_teacher(main_model, fixed_tokens, source_config_sha256=main_config_sha)
    export_path = write_teacher_export(export, args.artifact_dir / "teacher_export.json")

    levels = int(np.log2(args.context_length))
    rounds = main_model.rounds
    _, targets = teacher_to_score_and_targets(export, voices=args.voices)

    random = np.random.default_rng(args.sidecar_seed)
    feature_vector = export.feature_vector()
    controls = {
        "conditioned": feature_vector,
        "shuffled-teacher": random.permutation(feature_vector),
        "unconditioned": np.zeros_like(feature_vector),
    }

    results = {}
    for control_name, control_vector in controls.items():
        model, training_log = _train_sidecar(
            control_vector, targets, steps=args.sidecar_steps, levels=levels, rounds=rounds, seed=args.sidecar_seed
        )
        predictions = model.predict(Tensor(control_vector[None, :]), len(targets.pitch_bucket), levels, rounds)
        packet = compute_feedback_packet(predictions, targets, export, levels=levels, control=control_name)
        score = decode_score(predictions, voices=args.voices, levels=levels, tempo_bpm=60.0 + 40.0 * targets.tempo_bucket / 15.0)
        midi_path = args.artifact_dir / f"composition.{control_name}.mid"
        midi_path.write_bytes(score_to_midi_bytes(score))
        wav_path = None
        if args.render_wav:
            wav_path = render_wav(score, args.artifact_dir / f"composition.{control_name}.wav")
        results[control_name] = {
            "training": training_log,
            "feedback_packet": packet.to_dict(),
            "midi_path": str(midi_path),
            "wav_path": str(wav_path) if wav_path else None,
            "event_count": score.event_count(),
        }

    gate_report = {
        "conditioned_accuracy": results["conditioned"]["feedback_packet"]["structural_prediction_accuracy"],
        "shuffled_accuracy": results["shuffled-teacher"]["feedback_packet"]["structural_prediction_accuracy"],
        "unconditioned_accuracy": results["unconditioned"]["feedback_packet"]["structural_prediction_accuracy"],
    }
    gate_report["h2_falsified_at_this_scale"] = not (
        gate_report["conditioned_accuracy"] > gate_report["shuffled_accuracy"]
    )

    update_result = None
    if args.apply_feedback:
        checkpoint_path = args.artifact_dir / "main_model_reference.safetensors"
        from tinygrad.nn.state import get_state_dict, safe_save

        safe_save(
            {f"model.{key}": value for key, value in get_state_dict(main_model).items()},
            str(checkpoint_path),
        )
        update = apply_gated_music_feedback(
            model=main_model,
            checkpoint_path=checkpoint_path,
            corpus=corpus,
            context_length=args.context_length,
            conditioned_accuracy=gate_report["conditioned_accuracy"],
            shuffled_accuracy=gate_report["shuffled_accuracy"],
        )
        update_result = update.to_dict()

    record = {
        "status": "complete",
        "corpus_sha256": corpus.metadata.sha256,
        "main_model": spec.to_dict(),
        "main_model_parameters": main_params,
        "teacher_export_path": str(export_path),
        "results_by_control": results,
        "gate": gate_report,
        "gated_update": update_result,
    }
    _write_json(args.artifact_dir / "cycle_record.json", record)
    print(json.dumps(gate_report, indent=2, sort_keys=True))
    return record


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--enable",
        action="store_true",
        required=True,
        help="required opt-in: the sidecar refuses to run without this flag",
    )
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--corpus-path", type=Path, required=True)
    parser.add_argument("--corpus-bytes", type=int, default=200_000)
    parser.add_argument("--corpus-seed", type=int, default=20260720)
    parser.add_argument("--context-length", type=int, default=128)
    parser.add_argument("--main-arm", choices=("complex", "real", "nonrecursive", "phase-destroyed"), default="complex")
    parser.add_argument("--main-width", type=int, default=20)
    parser.add_argument("--main-rounds", type=int, default=2)
    parser.add_argument("--main-model-seed", type=int, default=20260720)
    parser.add_argument("--sidecar-seed", type=int, default=20260720)
    parser.add_argument("--sidecar-steps", type=int, default=200)
    parser.add_argument("--voices", type=int, default=2)
    parser.add_argument("--render-wav", action=argparse.BooleanOptionalAction, default=True)
    parser.add_argument("--apply-feedback", action="store_true")
    args = parser.parse_args(argv)
    args.artifact_dir = args.artifact_dir.expanduser().resolve()
    args.corpus_path = args.corpus_path.expanduser().resolve()
    return args


def main() -> None:
    run_cycle(parse_args())


if __name__ == "__main__":
    main()

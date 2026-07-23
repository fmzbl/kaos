from __future__ import annotations

import struct
import unittest
import wave
from pathlib import Path
from tempfile import TemporaryDirectory

import numpy as np
from tinygrad import Tensor
from tinygrad.nn.optim import Adam
from tinygrad.nn.state import get_state_dict

from .compiler import ensure_compiler
from .complex_path import build_path_model
from .music.adapter import MusicFeedbackAdapter
from .music.events import (
    HarmonyEvent,
    InvalidScoreError,
    MAX_EVENTS,
    NoteEvent,
    Score,
    SynthControl,
    validate_score,
)
from .music.feedback import compute_feedback_packet, decode_score
from .music.midi import score_to_midi_bytes
from .music.sidecar_model import SidecarModel, loss_against_targets
from .music.synth import render_wav
from .music.target import teacher_to_score_and_targets
from .music.teacher import export_teacher

ensure_compiler()


def _tiny_score() -> Score:
    synth = {
        0: SynthControl("sine", 5.0, 20.0, 0.6, 50.0, 4000.0, 0.7, -0.3),
        1: SynthControl("saw", 5.0, 20.0, 0.6, 50.0, 4000.0, 0.7, 0.3),
    }
    return Score(
        tempo_bpm=120.0,
        voices=2,
        notes=(
            NoteEvent(voice=0, pitch=60, velocity=90, start_tick=0, duration_ticks=240),
            NoteEvent(voice=1, pitch=64, velocity=80, start_tick=0, duration_ticks=240),
        ),
        harmony=(HarmonyEvent(start_tick=0, root_pitch_class=0, quality="major"),),
        synth=synth,
    )


class EventValidationTests(unittest.TestCase):
    def test_valid_score_passes(self) -> None:
        validate_score(_tiny_score())

    def test_out_of_range_pitch_is_rejected(self) -> None:
        bad = Score(
            tempo_bpm=120.0,
            voices=1,
            notes=(NoteEvent(voice=0, pitch=200, velocity=90, start_tick=0, duration_ticks=100),),
        )
        with self.assertRaises(InvalidScoreError):
            validate_score(bad)

    def test_event_count_cap_is_enforced(self) -> None:
        notes = tuple(
            NoteEvent(voice=0, pitch=60, velocity=90, start_tick=index * 10, duration_ticks=10)
            for index in range(MAX_EVENTS + 1)
        )
        with self.assertRaises(InvalidScoreError):
            validate_score(Score(tempo_bpm=120.0, voices=1, notes=notes))

    def test_voice_out_of_declared_range_is_rejected(self) -> None:
        bad = Score(
            tempo_bpm=120.0,
            voices=1,
            notes=(NoteEvent(voice=5, pitch=60, velocity=90, start_tick=0, duration_ticks=100),),
        )
        with self.assertRaises(InvalidScoreError):
            validate_score(bad)


class MidiRenderTests(unittest.TestCase):
    def test_midi_header_and_track_count(self) -> None:
        payload = score_to_midi_bytes(_tiny_score())
        self.assertEqual(payload[:4], b"MThd")
        _, format_type, track_count, division = struct.unpack(">IHHH", payload[4:14])
        self.assertEqual(format_type, 1)
        self.assertEqual(track_count, 1 + _tiny_score().voices)
        self.assertGreater(division, 0)

    def test_midi_rejects_invalid_score(self) -> None:
        bad = Score(tempo_bpm=1000.0, voices=1)
        with self.assertRaises(InvalidScoreError):
            score_to_midi_bytes(bad)


class WavRenderTests(unittest.TestCase):
    def test_wav_bounds_and_determinism(self) -> None:
        with TemporaryDirectory() as directory:
            first = render_wav(_tiny_score(), Path(directory) / "a.wav")
            second = render_wav(_tiny_score(), Path(directory) / "b.wav")
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with wave.open(str(first), "rb") as handle:
                self.assertEqual(handle.getnchannels(), 2)
                self.assertEqual(handle.getsampwidth(), 2)
                frames = handle.readframes(handle.getnframes())
            samples = np.frombuffer(frames, dtype=np.int16)
            self.assertTrue(np.isfinite(samples).all())
            self.assertLess(int(np.abs(samples).max()), 32768)


class TeacherExportTests(unittest.TestCase):
    def test_teacher_export_is_deterministic_and_detached(self) -> None:
        Tensor.manual_seed(4)
        model, spec, _ = build_path_model("complex", context_length=16, width=6, ffn_hidden=12)
        tokens = np.arange(16, dtype=np.int64)[None, :] % 32
        first = export_teacher(model, tokens, source_config_sha256="deadbeef")
        second = export_teacher(model, tokens, source_config_sha256="deadbeef")
        self.assertEqual(first.to_dict(), second.to_dict())
        # Nothing in the export is a tinygrad Tensor: it is pure Python/NumPy.
        for value in first.to_dict().values():
            self.assertNotIsInstance(value, Tensor)


class SidecarModelTests(unittest.TestCase):
    def _teacher_and_targets(self):
        Tensor.manual_seed(6)
        model, _, _ = build_path_model("complex", context_length=16, width=6, ffn_hidden=12, rounds=2)
        tokens = np.arange(16, dtype=np.int64)[None, :] % 32
        export = export_teacher(model, tokens, source_config_sha256="abc123")
        _, targets = teacher_to_score_and_targets(export, voices=2)
        return export, targets

    def test_shapes_and_finite_loss(self) -> None:
        export, targets = self._teacher_and_targets()
        levels = int(np.log2(16))
        rounds = len(export.round_radius_mean)
        Tensor.manual_seed(1)
        sidecar = SidecarModel(feature_width=len(export.feature_vector()))
        predictions = sidecar.predict(
            Tensor(export.feature_vector()[None, :]), len(targets.pitch_bucket), levels, rounds
        )
        self.assertEqual(predictions["pitch"].shape[1], len(targets.pitch_bucket))
        loss = loss_against_targets(predictions, targets, len(targets.pitch_bucket), rounds)
        self.assertTrue(np.isfinite(float(loss.item())))

    def test_conditioned_sidecar_beats_shuffled_after_training(self) -> None:
        export, targets = self._teacher_and_targets()
        levels = int(np.log2(16))
        rounds = len(export.round_radius_mean)
        steps = len(targets.pitch_bucket)

        def train(feature_vector: np.ndarray) -> dict:
            Tensor.manual_seed(2)
            sidecar = SidecarModel(feature_width=len(feature_vector))
            optimizer = Adam(sidecar.optimizer_parameters(), lr=5e-3)
            feature_tensor = Tensor(feature_vector[None, :])
            for _ in range(150):
                with Tensor.train():
                    optimizer.zero_grad()
                    predictions = sidecar.predict(feature_tensor, steps, levels, rounds)
                    loss = loss_against_targets(predictions, targets, steps, rounds)
                    loss.backward()
                    optimizer.step()
            final_predictions = sidecar.predict(feature_tensor, steps, levels, rounds)
            packet = compute_feedback_packet(final_predictions, targets, export, levels=levels, control="test")
            return packet.structural_prediction_accuracy

        conditioned_accuracy = train(export.feature_vector())
        random = np.random.default_rng(0)
        shuffled_accuracy = train(random.permutation(export.feature_vector()))
        self.assertGreaterEqual(conditioned_accuracy, shuffled_accuracy)

    def test_decoded_score_is_valid(self) -> None:
        export, targets = self._teacher_and_targets()
        levels = int(np.log2(16))
        rounds = len(export.round_radius_mean)
        Tensor.manual_seed(3)
        sidecar = SidecarModel(feature_width=len(export.feature_vector()))
        predictions = sidecar.predict(
            Tensor(export.feature_vector()[None, :]), len(targets.pitch_bucket), levels, rounds
        )
        score = decode_score(predictions, voices=2, levels=levels, tempo_bpm=120.0)
        validate_score(score)


class AdapterGateTests(unittest.TestCase):
    def test_adapter_is_identity_at_initialization(self) -> None:
        Tensor.manual_seed(8)
        adapter = MusicFeedbackAdapter(width=12, rank=3)
        state = Tensor.randn(2, 5, 12)
        np.testing.assert_allclose(adapter(state).numpy(), state.numpy(), atol=1e-6)

    def test_main_model_parameters_are_excluded_from_adapter_optimizer(self) -> None:
        Tensor.manual_seed(9)
        model, _, _ = build_path_model("complex", context_length=16, width=6, ffn_hidden=12)
        adapter = MusicFeedbackAdapter(model.repr_width)
        main_ids = {id(value) for value in model.optimizer_parameters()}
        adapter_ids = {id(value) for value in adapter.optimizer_parameters()}
        self.assertEqual(main_ids & adapter_ids, set())

    def test_gate_rejects_when_teacher_signal_is_uninformative(self) -> None:
        from .music.adapter import apply_gated_music_feedback
        from .data import ByteCorpus
        from .synthetic_data import write_synthetic_corpus

        with TemporaryDirectory() as directory:
            corpus_path = write_synthetic_corpus(Path(directory) / "corpus.bin", 4096, 1)
            corpus = ByteCorpus(corpus_path, split=(0.5, 0.25, 0.25))
            Tensor.manual_seed(10)
            model, _, _ = build_path_model("complex", context_length=16, width=6, ffn_hidden=12)
            checkpoint = Path(directory) / "reference.safetensors"
            from tinygrad.nn.state import safe_save

            safe_save(
                {f"model.{key}": value for key, value in get_state_dict(model).items()},
                str(checkpoint),
            )
            result = apply_gated_music_feedback(
                model=model,
                checkpoint_path=checkpoint,
                corpus=corpus,
                context_length=16,
                conditioned_accuracy=0.30,
                shuffled_accuracy=0.29,
                min_structural_margin=0.02,
            )
            self.assertFalse(result.gate_opened)
            self.assertFalse(result.promoted)
            self.assertEqual(result.checkpoint_sha256_before, result.checkpoint_sha256_after)


class CliDefaultsTests(unittest.TestCase):
    def test_cli_requires_explicit_enable_flag(self) -> None:
        from . import music_sidecar

        with self.assertRaises(SystemExit):
            music_sidecar.parse_args([])


if __name__ == "__main__":
    unittest.main()

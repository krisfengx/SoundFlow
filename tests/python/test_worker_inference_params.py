from __future__ import annotations

import copy
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

import yaml

if __package__:
    from . import _bootstrap as _worker_test_bootstrap
else:
    import _bootstrap as _worker_test_bootstrap

import worker_infer
import worker_models


class InferenceParameterVersionTests(unittest.TestCase):
    """A task created by a newer app must not run against an older worker silently."""

    def test_future_version_is_rejected_with_an_actionable_message(self):
        with self.assertRaises(worker_infer.InferenceParameterError) as caught:
            worker_infer.normalize_inference_params({"batch_size": 1}, 5)
        self.assertIn("newer than this worker supports", str(caught.exception))
        self.assertIn("Update the application", str(caught.exception))

    def test_legacy_and_current_versions_are_still_accepted(self):
        for version in (None, 1, 2, 3, 4):
            self.assertIsInstance(worker_infer.normalize_inference_params({}, version), dict)


class InferenceParameterBoundsTests(unittest.TestCase):
    """Out-of-range values must be rejected before the separator is constructed.

    These bounds apply to every inferenceParams version, not just v4 snapshots,
    so a task restored from history cannot feed a clamped or nonsensical value
    to the core.
    """

    def test_legal_values_pass_through_unchanged(self):
        params = {"aggression": 5, "post_process_threshold": 0.2,
                  "chunk_size": 262144, "overlap_size": 12000, "batch_size": 2}
        self.assertEqual(worker_infer._sanitize_runtime_inference_params(params), params)

    def test_overlap_size_without_chunk_size_is_allowed(self):
        # The model config supplies the effective chunk later, so a standalone
        # overlap value is still legitimate.
        self.assertEqual(
            worker_infer._sanitize_runtime_inference_params({"overlap_size": 441000}),
            {"overlap_size": 441000},
        )

    def test_out_of_range_values_are_rejected_before_device_setup(self):
        for params, fragment in [
            ({"aggression": 101}, "aggression"),
            ({"aggression": 999}, "aggression"),
            ({"post_process_threshold": 2}, "post_process_threshold"),
            ({"post_process_threshold": 1.5}, "post_process_threshold"),
        ]:
            with self.subTest(params=params):
                with self.assertRaises(worker_infer.InferenceParameterError) as caught:
                    worker_infer._sanitize_runtime_inference_params(params)
                self.assertIn(fragment, str(caught.exception))

    def test_boundary_values_are_accepted(self):
        for params in [
            {"aggression": 0}, {"aggression": 100},
            {"post_process_threshold": 0}, {"post_process_threshold": 1},
            # overlap_size is a raw sample count here, not bounded by chunk_size.
            {"chunk_size": 261120, "overlap_size": 350208},
        ]:
            with self.subTest(params=params):
                self.assertIsInstance(worker_infer._sanitize_runtime_inference_params(params), dict)

    def test_non_positive_values_are_still_dropped_not_rejected(self):
        self.assertEqual(
            worker_infer._sanitize_runtime_inference_params({"batch_size": 0, "aggression": -1}),
            {},
        )


class InferenceParameterCompatibilityTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.config_path = self.root / "model.yaml"
        self.config = {
            "audio": {"chunk_size": 588800},
            "inference": {"chunk_size": 882000, "num_overlap": 2, "batch_size": 1},
        }
        self.entry = types.SimpleNamespace(model_type="bs_roformer")
        self.pymss_config = types.ModuleType("pymss.config")
        self.pymss_config.load_config = lambda path: yaml.safe_load(Path(path).read_text(encoding="utf-8"))
        self.pymss_config.to_plain = lambda config: config
        pymss = types.ModuleType("pymss")
        pymss.config = self.pymss_config
        modules = mock.patch.dict(sys.modules, {"pymss": pymss, "pymss.config": self.pymss_config})
        modules.start()
        self.addCleanup(modules.stop)
        self.write_config()

    def write_config(self) -> None:
        self.config_path.write_text(yaml.safe_dump(self.config), encoding="utf-8")

    def runtime_params(self, params=None, model_type="bs_roformer"):
        return worker_infer._enrich_inference_params_for_model(
            model_type=model_type,
            config_path=str(self.config_path),
            inference_params=params or {},
        )

    def test_model_defaults_prefer_inference_chunk_size(self) -> None:
        defaults = worker_models.resolve_default_inference_params(
            self.entry, self.root / "model.ckpt", self.config_path,
        )
        self.assertEqual(defaults["chunk_size"], 882000)
        self.assertEqual(defaults["num_overlap"], 2)

    def test_mdx_chunk_validation_rejects_custom_size_before_constructing_a_model(self) -> None:
        from worker_inference_constraints import InferenceParameterError

        self.config["audio"] = {"hop_length": 1024, "n_fft": 8192, "chunk_size": 261120}
        self.config["model"] = {"num_scales": 5, "scale": [2, 2]}
        self.config["inference"].pop("chunk_size")
        self.write_config()
        meta = worker_models.resolve_inference_param_meta(types.SimpleNamespace(model_type="mdx23c"), self.config_path)
        self.assertEqual(meta["chunkSizeConstraint"], {"step": 32768, "offset": 31744, "min": 31744})
        with self.assertRaisesRegex(InferenceParameterError, "457728 or 490496"):
            self.runtime_params({"chunk_size": 465920, "overlap_size": 350208}, "mdx23c")
        for chunk in [261120, 457728, 490496]:
            params = self.runtime_params({"chunk_size": chunk, "overlap_size": 350208}, "mdx23c")
            self.assertEqual(params, {"chunk_size": chunk, "overlap_size": 350208})
        self.assertNotIn("chunk_size", self.runtime_params({}, "mdx23c"))

    def test_recommended_sample_step_uses_architecture_hop(self) -> None:
        cases = (
            ("bs_roformer", {"model": {"stft_hop_length": 512, "fft_size": 2048}}, 512, "model.stft_hop_length"),
            ("bs_roformer", {"audio": {"hop_length": 441}, "model": {"fft_size": 2048}}, 441, "audio.hop_length"),
            ("mdx23c", {"audio": {"hop_length": 1024}}, 1024, "audio.hop_length"),
            ("scnet", {"model": {"hop_size": 960}}, 960, "model.hop_size"),
            ("bandit_v2", {"kwargs": {"hop_length": 256}}, 256, "kwargs.hop_length"),
            ("apollo", {"model": {"sr": 44100, "win": 20}}, 441, "model.sr+win"),
        )
        for model_type, config, expected_step, expected_source in cases:
            with self.subTest(model_type=model_type):
                self.config_path.write_text(yaml.safe_dump(config), encoding="utf-8")
                entry = types.SimpleNamespace(model_type=model_type)
                meta = worker_models.resolve_inference_param_meta(entry, self.config_path)
                self.assertEqual(meta, {
                    "recommendedSampleStep": expected_step,
                    "source": expected_source,
                })

    def test_recommended_sample_step_ignores_vr_and_invalid_values(self) -> None:
        self.config_path.write_text(yaml.safe_dump({"model": {"hop_size": 0, "fft_size": -1}}), encoding="utf-8")
        self.assertEqual(
            worker_models.resolve_inference_param_meta(types.SimpleNamespace(model_type="scnet"), self.config_path),
            {},
        )
        self.assertEqual(
            worker_models.resolve_inference_param_meta(types.SimpleNamespace(model_type="vr"), self.config_path),
            {},
        )

    def test_roformer_does_not_fall_back_to_fft_or_training_hop(self) -> None:
        self.config_path.write_text(yaml.safe_dump({
            "model": {"fft_size": 2048, "multi_stft_hop_size": 147},
            "audio": {"n_fft": 2048},
        }), encoding="utf-8")
        self.assertEqual(
            worker_models.resolve_inference_param_meta(types.SimpleNamespace(model_type="bs_roformer"), self.config_path),
            {},
        )

    def test_separator_receives_overlap_for_the_effective_chunk_size(self) -> None:
        separator_type = mock.Mock()
        resolved = {
            "model_type": "bs_roformer",
            "model_path": str(self.root / "model.ckpt"),
            "config_path": str(self.config_path),
        }
        payload = {
            "model": "bs_roformer", "download": False,
            "output": str(self.root / "output"),
            "inferenceParamsVersion": 2, "inferenceParams": {},
        }
        with (
            mock.patch.object(worker_infer, "_resolve_separator_device", return_value=("cpu", [0], "CPU")),
            mock.patch.object(worker_infer, "_studio_separator_type", return_value=separator_type),
            mock.patch.object(worker_infer, "_resolve_studio_model", return_value=resolved),
            mock.patch.object(worker_infer, "emit"),
        ):
            worker_infer._prepare_separator(
                payload=payload, task_id="separation-1", logger=mock.Mock(), progress_callback=None,
            )

        self.assertEqual(separator_type.call_args.kwargs["inference_params"], {"overlap_size": 441000})
        self.assertEqual(payload["inferenceParams"], {})

    def test_catalog_target_override_reaches_separator_without_affecting_user_models(self) -> None:
        name = "model_mel_band_roformer_ep_0_sdr_11.4805.ckpt"
        weights = self.root / "model.ckpt"
        weights.touch()
        self.config["training"] = {
            "instruments": ["Vocals", "Instrumental"],
            "target_instrument": "Instrumental",
        }
        self.write_config()
        original_config = self.config_path.read_bytes()
        catalog = {
            "name": name, "model_type": "mel_band_roformer", "supported": True,
            "relpath": weights.name, "config_relpath": self.config_path.name,
        }
        user = worker_models.RegisteredUserModelEntry(
            name=name, model_type="mel_band_roformer",
            model_path=str(weights), config_path=str(self.config_path),
        )
        cases = (
            (worker_models.ModelEntry.from_dict({**catalog, "target_instrument_override": "Vocals"}), "Vocals"),
            (worker_models.ModelEntry.from_dict(catalog), None),
            (user, None),
        )
        for entry, expected_override in cases:
            with self.subTest(source=type(entry).__name__, override=expected_override):
                separator_type = mock.Mock()
                with (
                    mock.patch.object(worker_infer, "get_any_model_entry", return_value=entry),
                    mock.patch.object(worker_infer, "_resolve_separator_device", return_value=("cpu", [0], "CPU")),
                    mock.patch.object(worker_infer, "_studio_separator_type", return_value=separator_type),
                    mock.patch.object(worker_infer, "emit"),
                ):
                    worker_infer._prepare_separator(
                        payload={
                            "model": name, "modelDir": str(self.root), "download": False,
                            "output": str(self.root / "output"),
                            "inferenceParamsVersion": 2, "inferenceParams": {},
                        },
                        task_id="separation-1", logger=mock.Mock(), progress_callback=None,
                    )

                kwargs = separator_type.call_args.kwargs
                self.assertEqual(kwargs.get("target_instrument_override"), expected_override)
                if expected_override is None:
                    self.assertNotIn("target_instrument_override", kwargs)
                self.assertEqual(kwargs["model_path"], str(weights))
                self.assertEqual(self.config_path.read_bytes(), original_config)

    def test_legacy_separator_accepts_an_unused_target_override(self) -> None:
        class ConstructorReached(Exception):
            pass

        class LegacySeparator:
            def __init__(self, *, model_type):
                raise ConstructorReached(model_type)

        sys.modules["pymss"].MSSeparator = LegacySeparator
        with mock.patch("worker_rocm.configure_rocm_runtime"), self.assertRaisesRegex(ConstructorReached, "roformer"):
            worker_infer._studio_separator_type()(model_type="roformer", target_instrument_override=None)

    def test_legacy_separator_rejects_a_required_override_before_gpu_initialization(self) -> None:
        class LegacySeparator:
            def __init__(self, *, model_type):
                self.model_type = model_type

        sys.modules["pymss"].MSSeparator = LegacySeparator
        with mock.patch("worker_rocm.configure_rocm_runtime") as configure:
            with self.assertRaisesRegex(RuntimeError, "Update the kernel") as caught:
                worker_infer._studio_separator_type()(model_type="roformer", target_instrument_override="Vocals")
            configure.assert_not_called()
        with mock.patch.object(worker_infer, "emit_error", return_value=1) as emit_error:
            self.assertEqual(worker_infer._emit_inference_error(caught.exception, "legacy-task"), 1)
            self.assertEqual(emit_error.call_args.args[0], "RUNTIME_OUTDATED")
            self.assertFalse(emit_error.call_args.kwargs.get("recoverable", False))

    def test_modern_separator_retains_a_required_override(self) -> None:
        class ConstructorReached(Exception):
            pass

        class ModernSeparator:
            def __init__(self, *, target_instrument_override, **kwargs):
                raise ConstructorReached(target_instrument_override)

        sys.modules["pymss"].MSSeparator = ModernSeparator
        with mock.patch("worker_rocm.configure_rocm_runtime"), self.assertRaisesRegex(ConstructorReached, "Vocals"):
            worker_infer._studio_separator_type()(model_type="roformer", target_instrument_override="Vocals")

    def test_audio_chunk_size_is_used_when_inference_chunk_size_is_missing_or_null(self) -> None:
        for include_null in (False, True):
            with self.subTest(include_null=include_null):
                self.config["inference"].pop("chunk_size", None)
                if include_null:
                    self.config["inference"]["chunk_size"] = None
                self.write_config()
                defaults = worker_models.resolve_default_inference_params(
                    self.entry, self.root / "model.ckpt", self.config_path,
                )
                self.assertEqual(defaults["chunk_size"], 588800)
                self.assertEqual(self.runtime_params(), {"overlap_size": 294400})

    def test_user_chunk_size_and_overlap_count_take_precedence(self) -> None:
        for overrides, expected in (
            ({"chunk_size": 262144}, {"chunk_size": 262144, "overlap_size": 131072}),
            ({"chunk_size": 262144, "num_overlap": 8}, {"chunk_size": 262144, "overlap_size": 229376}),
            ({"num_overlap": 8}, {"overlap_size": 771750}),
            ({"num_overlap": 1}, {"overlap_size": 0}),
        ):
            with self.subTest(overrides=overrides):
                original = copy.deepcopy(overrides)
                self.assertEqual(self.runtime_params(overrides), expected)
                self.assertEqual(overrides, original)

    def test_explicit_overlap_size_is_preserved(self) -> None:
        overrides = {"chunk_size": 262144, "num_overlap": 8, "overlap_size": 12000}
        self.assertEqual(self.runtime_params(overrides), {"chunk_size": 262144, "overlap_size": 12000})

    def test_configured_overlap_size_is_left_to_the_core(self) -> None:
        self.config["inference"]["overlap_size"] = 12000
        self.write_config()
        self.assertEqual(self.runtime_params(), {})
        self.assertEqual(self.runtime_params({"num_overlap": 8}), {"overlap_size": 771750})

    def test_vr_and_apollo_do_not_receive_msst_overlap_conversion(self) -> None:
        for model_type in ("vr", "apollo"):
            with self.subTest(model_type=model_type):
                self.assertEqual(self.runtime_params({"num_overlap": 8}, model_type), {})


if __name__ == "__main__":
    unittest.main()

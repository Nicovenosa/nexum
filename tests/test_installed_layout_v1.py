#!/usr/bin/env python3
"""Contract tests for the versioned installed runtime layout."""

from __future__ import annotations

import os
import json
import hashlib
import subprocess
import shutil
import tempfile
import unittest
from pathlib import Path


CLI_ROOT = Path(__file__).resolve().parents[1]
LAYOUT_LIB = CLI_ROOT / "scripts" / "nexum-layout-lib"
PRODUCT_SIDECARS = (
    "nexum_hormiguero_sidecar",
    "nexum_memory_gateway",
    "nexum_experience",
    "nexum_nocturno",
    "nexum_workers",
    "nexum_providers",
)
KNOWN_GOOD_PROVIDER_MODELS = {
    "ollama_local": [
        "qwen3:1.7b",
        "qwen2.5:1.5b",
        "qwen2.5:0.5b",
        "moondream:latest",
    ],
    "claude_code": [
        "claude-opus-4-8",
        "claude-fable-5",
        "claude-opus-4-20250514",
        "claude-3-5-haiku-20241022",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-sonnet-5",
        "claude-sonnet-4-5-20250929",
        "claude-sonnet-4-6",
        "claude-3-7-sonnet-20250219",
        "claude-opus-4-5-20251101",
        "claude-opus-4-1-20250805",
        "claude-haiku-4-5-20251001",
        "claude-opus-4-6-thinking",
    ],
    "codex_cli": [
        "gpt-oss-120b-medium",
        "gpt-5.5",
        "gpt-image-1.5",
        "gpt-5.4-mini",
        "codex-auto-review",
        "gpt-5.6-terra",
        "gpt-5.4",
        "gpt-image-2",
        "gpt-5.6-luna",
        "gpt-5.3-codex-spark",
        "gpt-5.6-sol",
    ],
    "gemini_cli": [
        "gpt-oss-120b-medium",
        "gemini-pro-agent",
        "gemini-3.5-flash-extra-low",
        "gemini-3.1-flash-image",
        "gemini-3.1-pro-low",
        "claude-sonnet-4-6",
        "gemini-3-flash",
        "gemini-3-flash-agent",
        "gemini-3.5-flash-low",
        "gemini-3.1-flash-lite",
        "claude-opus-4-6-thinking",
    ],
    "opencode_zen": [
        "big-pickle",
        "deepseek-v4-flash-free",
        "ling-3.0-flash-free",
        "mimo-v2.5-free",
        "nemotron-3-ultra-free",
        "north-mini-code-free",
    ],
    "opencode_go": [
        "deepseek-v4-flash",
        "deepseek-v4-pro",
        "glm-5.1",
        "glm-5.2",
        "kimi-k2.6",
        "kimi-k2.7-code",
        "mimo-v2.5",
        "mimo-v2.5-pro",
        "minimax-m2.7",
        "minimax-m3",
        "qwen3.6-plus",
        "qwen3.7-max",
        "qwen3.7-plus",
    ],
    "mimo_code": [
        "mimo-v2.5",
        "mimo-v2.5-asr",
        "mimo-v2.5-pro",
        "mimo-v2.5-tts",
        "mimo-v2.5-tts-voiceclone",
        "mimo-v2.5-tts-voicedesign",
    ],
}
REQUIRED_DISPLAY_NAMES = {
    "codex_cli": "Codex / OpenAI",
    "claude_code": "Claude Code",
    "gemini_cli": "Gemini CLI",
    "mimo_code": "MiMo",
    "opencode_zen": "OpenCode Free",
    "opencode_go": "OpenCode Go",
}


class TestInstalledLayoutV1(unittest.TestCase):
    def test_provider_catalog_asset_is_in_git_archive(self) -> None:
        archive = subprocess.run(
            ["git", "archive", "--format=tar", "HEAD"],
            cwd=CLI_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(archive.returncode, 0, archive.stderr.decode())
        listing = subprocess.run(
            ["tar", "-tf", "-"],
            input=archive.stdout,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(listing.returncode, 0, listing.stderr.decode())
        self.assertIn(
            "config/provider-catalog-base.json",
            listing.stdout.decode().splitlines(),
        )

    def test_provider_catalog_asset_is_in_package(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw), "1.2.3")
            slot = artifact / "lib/nexum/1.2.3"
            self.assertTrue((slot / "provider-catalog-output.json").is_file())
            self.assertEqual(
                (slot / "provider-catalog-output.json").read_bytes(),
                (CLI_ROOT / "config/provider-catalog-base.json").read_bytes(),
            )

    def test_opencode_resolver_is_present_in_installed_slot(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_fixture(Path(raw), "1.2.3") / "lib/nexum/1.2.3"
            resolver = (
                slot
                / "libexec/nexum/providers/nexum_providers/provider_resolve.py"
            )
            self.assertTrue(resolver.is_file())
            package = json.loads((slot / "PACKAGE_MANIFEST.json").read_text())
            self.assertIn(
                "libexec/nexum/providers/nexum_providers/provider_resolve.py",
                {entry["path"] for entry in package["files"]},
            )

    def test_opencode_resolver_is_independent_of_checkout(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            slot = self._package_fixture(root, "1.2.3") / "lib/nexum/1.2.3"
            resolver = (
                slot
                / "libexec/nexum/providers/nexum_providers/provider_resolve.py"
            )
            isolated_home = root / "isolated-home"
            isolated_data = root / "isolated-data"
            isolated_home.mkdir()
            isolated_data.mkdir()
            probe = subprocess.run(
                ["python3", str(resolver), "opencode_zen"],
                cwd=Path("/"),
                env={
                    "HOME": str(isolated_home),
                    "XDG_DATA_HOME": str(isolated_data),
                    "PATH": os.environ["PATH"],
                },
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(probe.returncode, 1, probe.stderr)
            response = json.loads(probe.stdout)
            self.assertFalse(response["ok"])
            self.assertNotIn("ImportError", probe.stderr)

    def test_installed_provider_resolver_does_not_mutate_slot(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            slot = self._package_fixture(root, "1.2.3") / "lib/nexum/1.2.3"
            resolver = (
                slot
                / "libexec/nexum/providers/nexum_providers/provider_resolve.py"
            )
            before = {
                path.relative_to(slot)
                for path in slot.rglob("*")
                if path.is_file()
            }
            isolated_home = root / "isolated-home"
            isolated_data = root / "isolated-data"
            isolated_home.mkdir()
            isolated_data.mkdir()
            probe = subprocess.run(
                ["python3", str(resolver), "opencode_zen"],
                cwd=Path("/"),
                env={
                    "HOME": str(isolated_home),
                    "XDG_DATA_HOME": str(isolated_data),
                    "PATH": os.environ["PATH"],
                    "PYTHONDONTWRITEBYTECODE": "1",
                },
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertIn(probe.returncode, (0, 1), probe.stderr)
            after = {
                path.relative_to(slot)
                for path in slot.rglob("*")
                if path.is_file()
            }
            self.assertEqual(before, after)
            self.assertFalse(any("__pycache__" in path.parts for path in after))
            self.assertIn(
                '.env("PYTHONDONTWRITEBYTECODE", "1")',
                (CLI_ROOT / "nexum-tui/src/app/model_panel.rs").read_text(),
            )

    def test_provider_route_registry_is_in_package(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_fixture(Path(raw), "1.2.3") / "lib/nexum/1.2.3"
            route_path = slot / "provider-route-registry.json"
            self.assertTrue(route_path.is_file())
            registry = json.loads(route_path.read_text())
            self.assertEqual(registry["schema_version"], 1)
            self.assertEqual(len(registry["routes"]), 17)

    def test_provider_route_registry_exists_in_git_archive(self) -> None:
        archive = subprocess.run(
            ["git", "archive", "--format=tar", "HEAD"],
            cwd=CLI_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(archive.returncode, 0, archive.stderr.decode())
        listing = subprocess.run(
            ["tar", "-tf", "-"],
            input=archive.stdout,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(listing.returncode, 0, listing.stderr.decode())
        self.assertIn(
            "config/provider-route-registry.json",
            listing.stdout.decode().splitlines(),
        )

    def test_provider_route_registry_exists_in_staging(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            self.assertTrue((slot / "provider-route-registry.json").is_file())

    def test_provider_route_registry_exists_in_tarball(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw), "1.2.3")
            tarball = Path(f"{artifact}.tar.gz")
            listing = subprocess.run(
                ["tar", "-tzf", str(tarball)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(listing.returncode, 0, listing.stderr)
            self.assertIn(
                f"{artifact.name}/lib/nexum/1.2.3/provider-route-registry.json",
                listing.stdout.splitlines(),
            )

    def test_provider_route_registry_exists_in_installed_slot(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            artifact = self._package_fixture(root, "1.2.3")
            prefix = root / "prefix"
            installed = subprocess.run(
                [str(artifact / "nexum-install"), "--prefix", str(prefix)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(installed.returncode, 0, installed.stderr)
            self.assertTrue(
                (prefix / "lib/nexum/1.2.3/provider-route-registry.json").is_file()
            )

    def test_provider_route_registry_is_in_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            manifest = json.loads((slot / "MANIFEST.json").read_text())
            self.assertIn(
                "provider-route-registry.json", manifest["resource_sha256"]
            )
            self.assertIn(
                "provider-route-registry.json", manifest["required_resources"]
            )

    def test_provider_route_registry_is_in_package_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            manifest = json.loads((slot / "PACKAGE_MANIFEST.json").read_text())
            self.assertIn(
                "provider-route-registry.json", manifest["required_resources"]
            )
            self.assertIn(
                "provider-route-registry.json",
                {entry["path"] for entry in manifest["files"]},
            )

    def test_provider_route_registry_is_in_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            hashes = {
                line.split("\t", 1)[1]
                for line in (slot / "HASHES.tsv").read_text().splitlines()[1:]
            }
            self.assertIn("provider-route-registry.json", hashes)

    def test_provider_route_registry_hash_matches(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            path = slot / "provider-route-registry.json"
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            manifest = json.loads((slot / "MANIFEST.json").read_text())
            hashes = {
                relative: sha256
                for sha256, relative in (
                    line.split("\t", 1)
                    for line in (slot / "HASHES.tsv").read_text().splitlines()[1:]
                )
            }
            self.assertEqual(manifest["resource_sha256"][path.name], digest)
            self.assertEqual(hashes[path.name], digest)

    def test_activation_rejects_route_registry_schema_mismatch(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw), "1.2.3")
            slot = artifact / "lib/nexum/1.2.3"
            path = slot / "provider-route-registry.json"
            registry = json.loads(path.read_text())
            registry["schema_version"] = 999
            path.write_text(json.dumps(registry), encoding="utf-8")
            self._rehash_slot(slot)
            checked = self._validate_manifest(artifact, slot)
            self.assertNotEqual(checked.returncode, 0)
            self.assertIn("registry schema", checked.stderr)

    def test_activation_rejects_incomplete_route_registry(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw), "1.2.3")
            slot = artifact / "lib/nexum/1.2.3"
            path = slot / "provider-route-registry.json"
            registry = json.loads(path.read_text())
            registry["routes"] = registry["routes"][1:]
            path.write_text(json.dumps(registry), encoding="utf-8")
            self._rehash_slot(slot)
            checked = self._validate_manifest(artifact, slot)
            self.assertNotEqual(checked.returncode, 0)
            self.assertIn("registry completeness", checked.stderr)

    def test_provider_route_registry_loads_from_installed_prefix(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            self._assert_registry_contract(slot)

    def test_provider_route_registry_does_not_depend_on_checkout(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source_slot = self._package_slot(root / "source")
            isolated = root / "isolated-slot"
            shutil.copytree(source_slot, isolated)
            self.assertFalse((isolated / ".git").exists())
            self._assert_registry_contract(isolated)

    def test_provider_route_registry_does_not_depend_on_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            checked = subprocess.run(
                [
                    "bash",
                    str(CLI_ROOT / "scripts/nexum-package"),
                    "--validate-manifest",
                    str(slot),
                ],
                cwd=Path("/"),
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(checked.returncode, 0, checked.stderr)

    def test_provider_route_registry_does_not_depend_on_target(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            self.assertNotIn("target", slot.parts)
            self._assert_registry_contract(slot)

    def test_doctor_and_runtime_use_same_route_registry(self) -> None:
        doctor = (CLI_ROOT / "nexum-tui/src/doctor/checks.rs").read_text()
        runtime = (CLI_ROOT / "nexum-acp/src/provider/routes.rs").read_text()
        self.assertIn(
            "nexum_acp::provider::routes::validate_installed_registry()", doctor
        )
        self.assertIn("ProviderRouteRegistry::load_installed()", runtime)

    def test_doctor_and_runtime_use_same_schema(self) -> None:
        registry = json.loads(
            (CLI_ROOT / "config/provider-route-registry.json").read_text()
        )
        runtime = (CLI_ROOT / "nexum-acp/src/provider/routes.rs").read_text()
        self.assertEqual(registry["schema_version"], 1)
        self.assertIn("PROVIDER_ROUTE_SCHEMA_VERSION: u32 = 1", runtime)

    def test_doctor_and_runtime_use_same_validator(self) -> None:
        doctor = (CLI_ROOT / "nexum-tui/src/doctor/checks.rs").read_text()
        runtime = (CLI_ROOT / "nexum-acp/src/provider/routes.rs").read_text()
        self.assertIn("validate_installed_registry()", doctor)
        self.assertIn("validate_installed_registry()?", runtime)

    def test_route_registry_contains_all_visible_providers(self) -> None:
        registry, catalog = self._source_registry_and_catalog()
        self.assertEqual(
            {route["provider_id"] for route in registry["routes"]},
            {provider["provider_id"] for provider in catalog["providers"]},
        )

    def test_route_registry_contains_all_visible_models(self) -> None:
        registry, catalog = self._source_registry_and_catalog()
        routes = {route["provider_id"]: route for route in registry["routes"]}
        for provider in catalog["providers"]:
            route = routes[provider["provider_id"]]
            self.assertEqual(route["model_mapping"], "identity")
            for model in provider.get("models", []):
                self.assertTrue(model)

    def test_route_registry_contains_opencode_free(self) -> None:
        self._assert_source_route("opencode_zen")

    def test_route_registry_contains_opencode_go(self) -> None:
        self._assert_source_route("opencode_go")

    def test_route_registry_contains_mimo(self) -> None:
        self._assert_source_route("mimo_code")

    def test_route_registry_contains_codex(self) -> None:
        self._assert_source_route("codex_cli")

    def test_route_registry_contains_claude_code(self) -> None:
        self._assert_source_route("claude_code")

    def test_route_registry_contains_gemini_cli(self) -> None:
        self._assert_source_route("gemini_cli")

    def test_doctor_routes_present_passes(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            self.assertTrue(
                (self._package_slot(Path(raw)) / "provider-route-registry.json").is_file()
            )

    def test_doctor_routes_complete_passes(self) -> None:
        registry, catalog = self._source_registry_and_catalog()
        self.assertEqual(
            {route["provider_id"] for route in registry["routes"]},
            {provider["provider_id"] for provider in catalog["providers"]},
        )

    def test_doctor_routes_installed_independence_passes(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_slot(Path(raw))
            self.assertEqual(
                (slot / "provider-route-registry.json").parent.resolve(),
                slot.resolve(),
            )

    def test_doctor_cli_auth_modes_passes(self) -> None:
        registry, _ = self._source_registry_and_catalog()
        routes = {route["provider_id"]: route for route in registry["routes"]}
        for provider in ("codex_cli", "claude_code", "gemini_cli"):
            self.assertEqual(routes[provider]["auth_mode"], "cli_oauth")
        for provider in ("opencode_zen", "opencode_go"):
            self.assertEqual(routes[provider]["auth_mode"], "cli_account")

    def test_doctor_model_mappings_passes(self) -> None:
        registry, catalog = self._source_registry_and_catalog()
        routes = {route["provider_id"]: route for route in registry["routes"]}
        for provider in catalog["providers"]:
            self.assertIn(provider["provider_id"], routes)
            self.assertEqual(routes[provider["provider_id"]]["model_mapping"], "identity")

    def test_retired_claude_model_is_not_visible(self) -> None:
        _, catalog = self._source_registry_and_catalog()
        claude = next(
            provider
            for provider in catalog["providers"]
            if provider["provider_id"] == "claude_code"
        )
        self.assertNotIn("claude-sonnet-4-20250514", claude["models"])
        self.assertNotIn("claude-sonnet-4-20250514", claude["models_detected"])

    def test_current_claude_model_is_visible(self) -> None:
        _, catalog = self._source_registry_and_catalog()
        claude = next(
            provider
            for provider in catalog["providers"]
            if provider["provider_id"] == "claude_code"
        )
        self.assertIn("claude-sonnet-4-6", claude["models"])
        self.assertIn("claude-sonnet-4-6", claude["models_detected"])

    def test_claude_default_model_is_visible(self) -> None:
        _, catalog = self._source_registry_and_catalog()
        claude = next(
            provider
            for provider in catalog["providers"]
            if provider["provider_id"] == "claude_code"
        )
        self.assertIn("claude-sonnet-4-6", claude["models"])

    def test_claude_catalog_and_routes_match(self) -> None:
        registry, catalog = self._source_registry_and_catalog()
        claude = next(
            provider
            for provider in catalog["providers"]
            if provider["provider_id"] == "claude_code"
        )
        route = next(
            route
            for route in registry["routes"]
            if route["provider_id"] == "claude_code"
        )
        self.assertEqual(route["model_mapping"], "identity")
        self.assertTrue(claude["models"])
        self.assertEqual(claude["models"], claude["models_detected"])

    def test_gemini_cli_does_not_require_model_from_other_provider(self) -> None:
        _, catalog = self._source_registry_and_catalog()
        gemini = next(
            provider
            for provider in catalog["providers"]
            if provider["provider_id"] == "gemini_cli"
        )
        self.assertNotIn("gemini-2.5-pro", gemini["models"])

    def test_gemini_cli_current_model_is_visible(self) -> None:
        _, catalog = self._source_registry_and_catalog()
        gemini = next(
            provider
            for provider in catalog["providers"]
            if provider["provider_id"] == "gemini_cli"
        )
        self.assertIn("gemini-3-flash", gemini["models"])

    def test_gemini_cli_current_model_has_execution_mapping(self) -> None:
        registry, catalog = self._source_registry_and_catalog()
        gemini = next(
            provider
            for provider in catalog["providers"]
            if provider["provider_id"] == "gemini_cli"
        )
        route = next(
            route
            for route in registry["routes"]
            if route["provider_id"] == "gemini_cli"
        )
        self.assertIn("gemini-3-flash", gemini["models"])
        self.assertEqual(route["model_mapping"], "identity")

    def test_provider_catalog_manifest_entry_exists(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_fixture(Path(raw), "1.2.3") / "lib/nexum/1.2.3"
            overlay = json.loads((slot / "MANIFEST.json").read_text())
            package = json.loads((slot / "PACKAGE_MANIFEST.json").read_text())
            self.assertIn("provider-catalog-output.json", overlay["resource_sha256"])
            self.assertIn("provider-catalog-output.json", overlay["required_payload"])
            self.assertIn(
                "provider-catalog-output.json",
                {entry["path"] for entry in package["files"]},
            )

    def test_provider_catalog_hash_matches(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            slot = self._package_fixture(Path(raw), "1.2.3") / "lib/nexum/1.2.3"
            catalog = slot / "provider-catalog-output.json"
            digest = hashlib.sha256(catalog.read_bytes()).hexdigest()
            overlay = json.loads((slot / "MANIFEST.json").read_text())
            hashes = dict(
                line.split("\t", 1)[::-1]
                for line in (slot / "HASHES.tsv").read_text().splitlines()[1:]
            )
            self.assertEqual(overlay["resource_sha256"][catalog.name], digest)
            self.assertEqual(hashes[catalog.name], digest)

    def test_provider_catalog_loads_from_installed_prefix(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            slot = self._package_fixture(raw_path, "1.2.3") / "lib/nexum/1.2.3"
            catalog = json.loads((slot / "provider-catalog-output.json").read_text())
            self.assertEqual(len(catalog["providers"]), 18)

    def test_provider_catalog_does_not_depend_on_checkout(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            slot = self._package_fixture(raw_path, "1.2.3") / "lib/nexum/1.2.3"
            self.assertFalse((raw_path / "cli/.git").exists())
            self.assertFalse((raw_path / "cli/research").exists())
            self.assertEqual(
                json.loads((slot / "provider-catalog-output.json").read_text())[
                    "catalog_kind"
                ],
                "base",
            )

    def test_provider_catalog_does_not_depend_on_cwd(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            slot = self._package_fixture(raw_path, "1.2.3") / "lib/nexum/1.2.3"
            probe = subprocess.run(
                [
                    "python3",
                    "-c",
                    "import json,sys; print(len(json.load(open(sys.argv[1]))['providers']))",
                    str(slot / "provider-catalog-output.json"),
                ],
                cwd=Path("/"),
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(probe.returncode, 0, probe.stderr)
            self.assertEqual(probe.stdout.strip(), "18")

    def test_provider_catalog_does_not_depend_on_target(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            slot = self._package_fixture(raw_path, "1.2.3") / "lib/nexum/1.2.3"
            build_target = raw_path / "cli/target"
            build_target.rename(raw_path / "cli/target-preserved")
            self.assertTrue((slot / "provider-catalog-output.json").is_file())

    def test_provider_catalog_contains_known_good_provider_ids(self) -> None:
        catalog = json.loads(
            (CLI_ROOT / "config/provider-catalog-base.json").read_text()
        )
        ids = {provider["provider_id"] for provider in catalog["providers"]}
        self.assertTrue(set(KNOWN_GOOD_PROVIDER_MODELS).issubset(ids))

    def test_provider_catalog_models_match_known_good_baseline(self) -> None:
        catalog = json.loads(
            (CLI_ROOT / "config/provider-catalog-base.json").read_text()
        )
        providers = {
            provider["provider_id"]: provider["models"]
            for provider in catalog["providers"]
        }
        for provider_id, models in KNOWN_GOOD_PROVIDER_MODELS.items():
            self.assertEqual(providers[provider_id], models, provider_id)

    def test_opencode_free_excludes_unsupported_hy3_free_route(self) -> None:
        catalog = json.loads(
            (CLI_ROOT / "config/provider-catalog-base.json").read_text()
        )
        provider = next(
            item
            for item in catalog["providers"]
            if item["provider_id"] == "opencode_zen"
        )
        self.assertNotIn("hy3-free", provider["models"])
        self.assertIn("ling-3.0-flash-free", provider["models"])

    def _assert_visible(self, provider_id: str) -> None:
        catalog = json.loads(
            (CLI_ROOT / "config/provider-catalog-base.json").read_text()
        )
        providers = {
            provider["provider_id"]: provider for provider in catalog["providers"]
        }
        self.assertEqual(
            providers[provider_id]["display_name"],
            REQUIRED_DISPLAY_NAMES[provider_id],
        )
        self.assertTrue(providers[provider_id]["models"])

    def test_codex_is_visible(self) -> None:
        self._assert_visible("codex_cli")

    def test_claude_code_is_visible(self) -> None:
        self._assert_visible("claude_code")

    def test_gemini_cli_is_visible(self) -> None:
        self._assert_visible("gemini_cli")

    def test_mimo_is_visible(self) -> None:
        self._assert_visible("mimo_code")

    def test_opencode_free_is_visible(self) -> None:
        self._assert_visible("opencode_zen")

    def test_opencode_go_is_visible(self) -> None:
        self._assert_visible("opencode_go")

    def test_cli_authenticated_provider_not_filtered_by_missing_http_api_key(self) -> None:
        catalog = json.loads(
            (CLI_ROOT / "config/provider-catalog-base.json").read_text()
        )
        cli = [
            provider
            for provider in catalog["providers"]
            if provider["provider_id"] in {"codex_cli", "claude_code", "gemini_cli"}
        ]
        self.assertTrue(all(not provider["credential_detected"] for provider in cli))
        self.assertTrue(all(provider["models"] for provider in cli))

    def test_free_and_go_plans_are_not_collapsed(self) -> None:
        self.assertNotEqual(
            KNOWN_GOOD_PROVIDER_MODELS["opencode_zen"],
            KNOWN_GOOD_PROVIDER_MODELS["opencode_go"],
        )
        self.assertTrue(
            set(KNOWN_GOOD_PROVIDER_MODELS["opencode_zen"]).isdisjoint(
                KNOWN_GOOD_PROVIDER_MODELS["opencode_go"]
            )
        )

    def test_layout_contract_has_required_payload_and_relative_linux_targets(
        self,
    ) -> None:
        command = r"""
            source "$1"
            nexum_layout_required_payload
            printf 'ROOT=%s\n' "$(nexum_layout_version_root /prefix 1.2.3)"
            printf 'CURRENT=%s\n' "$(nexum_layout_current_path /prefix)"
            printf 'TARGET=%s\n' "$(nexum_layout_linux_launcher_target nexum)"
        """
        result = subprocess.run(
            ["bash", "-c", command, "layout-test", str(LAYOUT_LIB)],
            capture_output=True,
            text=True,
            check=False,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout.splitlines()[:21],
            [
                "nexum",
                "nexum-acp-host",
                "nexum-autologin-reconcile",
                "provider-catalog-output.json",
                "provider-catalog-base.json",
                "provider-route-registry.json",
                "reserved-models.json",
                "catalog-contract.json",
                "libexec/nexum/providers/nexum_providers",
                "src/nexum_hormiguero_sidecar",
                "src/nexum_memory_gateway",
                "src/nexum_experience",
                "src/nexum_nocturno",
                "src/nexum_workers",
                "src/nexum_providers",
                "schemas",
                "LICENSE",
                "NOTICE",
                "MANIFEST.json",
                "HASHES.tsv",
                "PACKAGE_MANIFEST.json",
            ],
        )
        self.assertIn("ROOT=/prefix/lib/nexum/1.2.3", result.stdout)
        self.assertIn("CURRENT=/prefix/lib/nexum/current", result.stdout)
        self.assertIn("TARGET=../lib/nexum/current/nexum", result.stdout)
        self.assertNotIn("HOME", LAYOUT_LIB.read_text(encoding="utf-8"))

    def test_package_creates_versioned_runtime_with_verified_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw) / "cli"
            dist = Path(raw) / "dist"
            self._make_packaging_fixture(root)

            result = subprocess.run(
                [str(root / "scripts" / "nexum-package"), "1.2.3", str(dist)],
                env=self._package_environment(),
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            prefix = dist / "nexum-1.2.3-linux-x86_64"
            version_root = prefix / "lib" / "nexum" / "1.2.3"
            self.assertTrue((prefix / "lib" / "nexum" / "current").is_symlink())
            self.assertEqual(
                (prefix / "lib" / "nexum" / "current").readlink(), Path("1.2.3")
            )
            for relative in self._required_payload():
                self.assertTrue((version_root / relative).exists(), relative)
            manifest = json.loads(
                (version_root / "PACKAGE_MANIFEST.json").read_text(encoding="utf-8")
            )
            self.assertTrue(
                {f"src/{sidecar}" for sidecar in PRODUCT_SIDECARS}.issubset(
                    set(manifest["required_payload"])
                )
            )
            for command in (
                "nexum",
                "nexum-acp-host",
                "nexum-autologin-reconcile",
            ):
                self.assertEqual(
                    (prefix / "bin" / command).readlink(),
                    Path(f"../lib/nexum/current/{command}"),
                )
            manifest_check = subprocess.run(
                [
                    "bash",
                    str(root / "scripts" / "nexum-package"),
                    "--validate-manifest",
                    str(version_root),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(manifest_check.returncode, 0, manifest_check.stderr)

    def test_package_uses_product_reserved_models_without_research(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw) / "cli"
            dist = Path(raw) / "dist"
            self._make_packaging_fixture(root)

            result = subprocess.run(
                [str(root / "scripts" / "nexum-package"), "1.2.3", str(dist)],
                env=self._package_environment(),
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            version_root = dist / "nexum-1.2.3-linux-x86_64/lib/nexum/1.2.3"
            product_policy = (
                root / "src/nexum_providers/catalog_gen/reserved-models.json"
            ).read_bytes()
            self.assertEqual(
                (version_root / "reserved-models.json").read_bytes(), product_policy
            )
            self.assertFalse((root / "research").exists())

    def test_install_rejects_a_tampered_runtime_before_activation(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw) / "cli"
            dist = Path(raw) / "dist"
            prefix = Path(raw) / "prefix"
            self._make_packaging_fixture(root)
            package = subprocess.run(
                [str(root / "scripts" / "nexum-package"), "1.2.3", str(dist)],
                env=self._package_environment(),
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(package.returncode, 0, package.stderr)
            artifact = dist / "nexum-1.2.3-linux-x86_64"
            version_root = artifact / "lib" / "nexum" / "1.2.3"
            (version_root / "provider-catalog-base.json").write_text("tampered\n")

            result = subprocess.run(
                [str(artifact / "nexum-install"), "--prefix", str(prefix)],
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("manifest", result.stderr.lower())
            self.assertFalse((prefix / "lib" / "nexum" / "1.2.3").exists())

    def test_manifest_rejects_self_consistent_required_payload_omission(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw) / "artifact", "1.2.3")
            version_root = artifact / "lib" / "nexum" / "1.2.3"
            manifest_path = version_root / "PACKAGE_MANIFEST.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            manifest["required_payload"].remove("NOTICE")
            manifest["files"] = [
                entry for entry in manifest["files"] if entry["path"] != "NOTICE"
            ]
            (version_root / "NOTICE").unlink()
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

            result = self._validate_manifest(artifact, version_root)

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("required", result.stderr.lower())

    def test_manifest_rejects_additional_and_traversal_entries(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw) / "artifact", "1.2.3")
            version_root = artifact / "lib" / "nexum" / "1.2.3"
            manifest_path = version_root / "PACKAGE_MANIFEST.json"
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            extra = version_root / "unexpected.py"
            extra.write_text("unexpected\n", encoding="utf-8")
            manifest["files"].append(
                {
                    "path": "unexpected.py",
                    "sha256": hashlib.sha256(extra.read_bytes()).hexdigest(),
                }
            )
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            additional = self._validate_manifest(artifact, version_root)
            self.assertNotEqual(additional.returncode, 0)
            self.assertIn("allowed", additional.stderr.lower())

            outside = version_root.parent / "outside"
            outside.write_text("outside\n", encoding="utf-8")
            manifest["files"][-1] = {
                "path": "../outside",
                "sha256": hashlib.sha256(outside.read_bytes()).hexdigest(),
            }
            extra.unlink()
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            traversal = self._validate_manifest(artifact, version_root)
            self.assertNotEqual(traversal.returncode, 0)
            self.assertIn("path", traversal.stderr.lower())

    def test_activation_rollback_and_precise_uninstall_preserve_data(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            prefix = raw_path / "prefix"
            first_artifact = self._package_fixture(raw_path / "first", "1.2.3")
            second_artifact = self._package_fixture(raw_path / "second", "2.0.0")

            for artifact in (first_artifact, second_artifact):
                result = subprocess.run(
                    [str(artifact / "nexum-install"), "--prefix", str(prefix)],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                self.assertEqual(result.returncode, 0, result.stderr)

            current = prefix / "lib" / "nexum" / "current"
            self.assertEqual(current.readlink(), Path("2.0.0"))
            self.assertEqual(
                (prefix / "bin" / "nexum").readlink(),
                Path("../lib/nexum/current/nexum"),
            )
            rollback = subprocess.run(
                [
                    str(second_artifact / "nexum-install"),
                    "--prefix",
                    str(prefix),
                    "--rollback",
                    "1.2.3",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(rollback.returncode, 0, rollback.stderr)
            self.assertEqual(current.readlink(), Path("1.2.3"))

            user_data = prefix / "data" / "nexum" / "settings.json"
            user_data.parent.mkdir(parents=True)
            user_data.write_text('{"preserve": true}\n', encoding="utf-8")
            uninstall_old = subprocess.run(
                [
                    str(second_artifact / "nexum-uninstall"),
                    "--prefix",
                    str(prefix),
                    "--version",
                    "2.0.0",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(uninstall_old.returncode, 0, uninstall_old.stderr)
            self.assertTrue((prefix / "lib" / "nexum" / "1.2.3").is_dir())
            self.assertFalse((prefix / "lib" / "nexum" / "2.0.0").exists())
            self.assertTrue(user_data.exists())

    def test_package_and_rollback_reject_unsafe_versions(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            root = raw_path / "cli"
            self._make_packaging_fixture(root)
            package = subprocess.run(
                [
                    str(root / "scripts" / "nexum-package"),
                    "../escape",
                    str(raw_path / "dist"),
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(package.returncode, 0)
            self.assertIn("invalid version", package.stderr.lower())

            artifact = self._package_fixture(raw_path / "artifact", "1.2.3")
            rollback = subprocess.run(
                [
                    str(artifact / "nexum-install"),
                    "--prefix",
                    str(raw_path / "prefix"),
                    "--rollback",
                    "../escape",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertNotEqual(rollback.returncode, 0)
            self.assertIn("invalid version", rollback.stderr.lower())

    def test_installed_binary_imports_all_product_sidecars_without_checkout(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            artifact = self._package_fixture(raw_path / "artifact", "1.2.3")
            prefix = raw_path / "prefix"
            install = subprocess.run(
                [str(artifact / "nexum-install"), "--prefix", str(prefix)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(install.returncode, 0, install.stderr)
            version_root = prefix / "lib" / "nexum" / "1.2.3"
            for sidecar in PRODUCT_SIDECARS:
                self.assertTrue((version_root / "src" / sidecar).is_dir(), sidecar)
            environment = dict(os.environ)
            environment.pop("PYTHONPATH", None)
            result = subprocess.run(
                [str(prefix / "bin" / "nexum"), "--verify-sidecars"],
                cwd=raw_path,
                env=environment,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_clean_archive_packages_installedlayout_v1(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            artifact = self._package_fixture(raw_path, "1.2.3+archive")
            self.assertFalse((raw_path / "cli" / ".git").exists())
            manifest = json.loads(
                (
                    artifact
                    / "lib"
                    / "nexum"
                    / "1.2.3+archive"
                    / "PACKAGE_MANIFEST.json"
                ).read_text(encoding="utf-8")
            )
            self.assertEqual(manifest["layout"], "InstalledLayoutV1")
            self.assertEqual(
                manifest["source_head"], self._package_environment()["NEXUM_SOURCE_HEAD"]
            )

    def test_slot_root_contains_required_binaries(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw), "1.2.3")
            slot = artifact / "lib" / "nexum" / "1.2.3"
            self.assertTrue((slot / "nexum").is_file())
            self.assertTrue((slot / "nexum-acp-host").is_file())
            self.assertFalse((slot / "bin" / "nexum").exists())

    def test_public_launcher_target_exists(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw), "1.2.3")
            launcher = artifact / "bin" / "nexum"
            self.assertTrue(launcher.is_symlink())
            self.assertTrue(launcher.resolve(strict=True).is_file())

    def test_public_launcher_target_executes(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            artifact = self._package_fixture(raw_path, "1.2.3")
            prefix = raw_path / "prefix"
            installed = subprocess.run(
                [str(artifact / "nexum-install"), "--prefix", str(prefix)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(installed.returncode, 0, installed.stderr)
            launched = subprocess.run(
                [str(prefix / "bin" / "nexum"), "--version"],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(launched.returncode, 0, launched.stderr)
            self.assertIn("nexum 1.2.3", launched.stdout)

    def test_manifest_matches_payload(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw), "1.2.3")
            slot = artifact / "lib" / "nexum" / "1.2.3"
            checked = self._validate_manifest(artifact, slot)
            self.assertEqual(checked.returncode, 0, checked.stderr)
            hashes = (slot / "HASHES.tsv").read_text(encoding="utf-8").splitlines()
            self.assertEqual(hashes[0], "sha256\tpath")
            self.assertGreater(len(hashes), 2)

    def test_licenses_are_packaged(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            artifact = self._package_fixture(Path(raw), "1.2.3")
            slot = artifact / "lib" / "nexum" / "1.2.3"
            self.assertEqual((slot / "LICENSE").read_bytes(), (CLI_ROOT / "LICENSE").read_bytes())
            self.assertEqual((slot / "NOTICE").read_bytes(), (CLI_ROOT / "NOTICE").read_bytes())

    def test_clean_rebuild_needs_no_manual_fixes(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            raw_path = Path(raw)
            artifact = self._package_fixture(raw_path, "1.2.3+clean")
            prefix = raw_path / "prefix"
            installed = subprocess.run(
                [str(artifact / "nexum-install"), "--prefix", str(prefix)],
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(installed.returncode, 0, installed.stderr)
            slot = prefix / "lib" / "nexum" / "1.2.3+clean"
            self.assertEqual(self._validate_manifest(artifact, slot).returncode, 0)
            self.assertTrue((prefix / "bin" / "nexum").resolve(strict=True).is_file())

    def _package_fixture(self, root: Path, version: str) -> Path:
        dist = root / "dist"
        self._make_packaging_fixture(root / "cli", version)
        result = subprocess.run(
            [str(root / "cli/scripts/nexum-package"), version, str(dist)],
            env=self._package_environment(),
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return dist / f"nexum-{version}-linux-x86_64"

    def _package_slot(self, root: Path, version: str = "1.2.3") -> Path:
        return self._package_fixture(root, version) / "lib/nexum" / version

    def _assert_registry_contract(self, slot: Path) -> None:
        registry = json.loads((slot / "provider-route-registry.json").read_text())
        catalog = json.loads((slot / "provider-catalog-output.json").read_text())
        self.assertEqual(registry["schema_version"], 1)
        route_ids = {route["provider_id"] for route in registry["routes"]}
        self.assertEqual(
            route_ids,
            {provider["provider_id"] for provider in catalog["providers"]},
        )

    @staticmethod
    def _source_registry_and_catalog() -> tuple[dict, dict]:
        return (
            json.loads(
                (CLI_ROOT / "config/provider-route-registry.json").read_text()
            ),
            json.loads((CLI_ROOT / "config/provider-catalog-base.json").read_text()),
        )

    def _assert_source_route(self, provider_id: str) -> None:
        registry, _ = self._source_registry_and_catalog()
        self.assertIn(
            provider_id,
            {route["provider_id"] for route in registry["routes"]},
        )

    @staticmethod
    def _rehash_slot(slot: Path) -> None:
        def digest(path: Path) -> str:
            return hashlib.sha256(path.read_bytes()).hexdigest()

        registry_path = slot / "provider-route-registry.json"
        overlay_path = slot / "MANIFEST.json"
        hashes_path = slot / "HASHES.tsv"
        package_path = slot / "PACKAGE_MANIFEST.json"

        overlay = json.loads(overlay_path.read_text())
        overlay["resource_sha256"][registry_path.name] = digest(registry_path)
        overlay_path.write_text(json.dumps(overlay), encoding="utf-8")

        rows = [
            line.split("\t", 1)
            for line in hashes_path.read_text().splitlines()[1:]
        ]
        replacements = {
            registry_path.name: digest(registry_path),
            overlay_path.name: digest(overlay_path),
        }
        hashes_path.write_text(
            "sha256\tpath\n"
            + "\n".join(
                f"{replacements.get(relative, sha256)}\t{relative}"
                for sha256, relative in rows
            )
            + "\n",
            encoding="utf-8",
        )

        package = json.loads(package_path.read_text())
        replacements["HASHES.tsv"] = digest(hashes_path)
        for entry in package["files"]:
            if entry["path"] in replacements:
                entry["sha256"] = replacements[entry["path"]]
        package_path.write_text(json.dumps(package), encoding="utf-8")

    @staticmethod
    def _validate_manifest(
        artifact: Path, version_root: Path
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                "bash",
                str(CLI_ROOT / "scripts" / "nexum-package"),
                "--validate-manifest",
                str(version_root),
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    @staticmethod
    def _required_payload() -> tuple[str, ...]:
        return (
            "nexum",
            "nexum-acp-host",
            "nexum-autologin-reconcile",
            "provider-catalog-output.json",
            "provider-catalog-base.json",
            "provider-route-registry.json",
            "reserved-models.json",
            "catalog-contract.json",
            "libexec/nexum/providers/nexum_providers",
            "src/nexum_providers",
            "schemas",
            "LICENSE",
            "NOTICE",
            "MANIFEST.json",
            "HASHES.tsv",
            "PACKAGE_MANIFEST.json",
        )

    @staticmethod
    def _package_environment() -> dict[str, str]:
        environment = dict(os.environ)
        environment["NEXUM_SOURCE_HEAD"] = "1" * 40
        environment["NEXUM_SOURCE_TREE"] = "2" * 40
        return environment

    @staticmethod
    def _make_packaging_fixture(root: Path, version: str = "1.2.3") -> None:
        (root / "scripts").mkdir(parents=True)
        (root / "config").mkdir()
        (root / "target" / "release").mkdir(parents=True)
        for script in (
            "nexum-package",
            "nexum-layout-lib",
            "nexum-autologin-reconcile",
            "nexum-install",
            "nexum-uninstall",
        ):
            shutil.copy2(CLI_ROOT / "scripts" / script, root / "scripts" / script)
        shutil.copy2(CLI_ROOT / "NOTICE", root / "NOTICE")
        shutil.copy2(CLI_ROOT / "LICENSE", root / "LICENSE")
        shutil.copy2(
            CLI_ROOT / "config" / "provider-catalog-base.json",
            root / "config" / "provider-catalog-base.json",
        )
        # Fuente única de la generación: es payload requerido, el fixture la
        # necesita igual que el catálogo base.
        shutil.copy2(
            CLI_ROOT / "config" / "catalog-contract.json",
            root / "config" / "catalog-contract.json",
        )
        shutil.copy2(
            CLI_ROOT / "config" / "provider-route-registry.json",
            root / "config" / "provider-route-registry.json",
        )
        shutil.copytree(
            CLI_ROOT / "src" / "nexum_providers",
            root / "src" / "nexum_providers",
        )
        for sidecar in PRODUCT_SIDECARS:
            if sidecar != "nexum_providers":
                shutil.copytree(CLI_ROOT / "src" / sidecar, root / "src" / sidecar)
        for name, body in {
            "nexum": (
                "#!/usr/bin/env bash\n"
                'if [[ "${1:-}" == "--verify-sidecars" ]]; then\n'
                '  ROOT="$(dirname "$(readlink -f "$0")")"\n'
                '  PYTHONPATH="$ROOT/src" python3 -c '
                "'import nexum_hormiguero_sidecar, nexum_memory_gateway, "
                "nexum_experience, nexum_nocturno, nexum_workers, nexum_providers'\n"
                "  exit $?\n"
                "fi\n"
                f"printf 'nexum {version}\\n'\n"
            ),
            "nexum-acp-host": "#!/usr/bin/env bash\nexit 0\n",
        }.items():
            target = root / "target" / "release" / name
            target.write_text(body, encoding="utf-8")
            target.chmod(0o755)


if __name__ == "__main__":
    unittest.main()

"""质量门防假通过合同；不运行 Cargo、数据库或真实压力测试。"""

import importlib.util
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("linux_gate", Path(__file__).with_name("run.py"))
gate = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = gate
spec.loader.exec_module(gate)
import acceptance
from acceptance import classify_cases, validate_postgres, validate_soak


class GateTests(unittest.TestCase):
    def test_ci_config_uses_the_shared_performance_section(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "config").mkdir()
            setting = root / "config/setting.json"
            config = {"output": str(root / "results"), "inputs": []}
            document = {"performance": {"linux_ci": config}}
            with patch.object(gate, "ROOT", root):
                with self.assertRaisesRegex(gate.MissingInput, r"performance\.linux_ci"):
                    gate.load_config()
                setting.write_text(json.dumps(document))
                self.assertEqual(gate.load_config(), (document, config))
                before = gate.input_fingerprint(document, config)
                config["command_timeout_seconds"] = 120
                self.assertNotEqual(before, gate.input_fingerprint(document, config))
                for invalid in ({"linux_ci": config}, {"performance": {}},
                                {"performance": []}, {"performance": {"linux_ci": []}}):
                    setting.write_text(json.dumps(invalid))
                    with self.subTest(document=invalid), self.assertRaisesRegex(
                            gate.MissingInput, r"performance\.linux_ci"):
                        gate.load_config()

    def test_performance_peer_fingerprints_resolve_project_relative_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "config").mkdir()
            (root / "peers").mkdir()
            peer = root / "peers/network"
            peer.write_text("original peer")
            live_peer = root / "peers/live"
            live_peer.write_text("original live peer")
            document = {"performance": {"network_peer": "peers/network", "live_peer": str(live_peer),
                                        "linux_ci": {"inputs": []}}}
            (root / "config/setting.json").write_text(json.dumps(document))
            with patch.object(gate, "ROOT", root):
                before = gate.input_fingerprint(document, {})
                peer.write_text("changed peer")
                self.assertNotEqual(before, gate.input_fingerprint(document, {}))
                before = gate.input_fingerprint(document, {})
                live_peer.write_text("changed live peer")
                self.assertNotEqual(before, gate.input_fingerprint(document, {}))
                for config in ({"inputs": ["peers/network"]}, {"tools": {"cargo": "peers/network"}}):
                    with self.subTest(config=config), self.assertRaisesRegex(ValueError, "absolute paths"):
                        gate.input_fingerprint(document, config)

    def test_default_targets_preserve_workspace_resolved_features(self):
        target = {"package": "dever-backend-bridge", "selector": ["--test", "backend_bridge"],
                  "features": ["embedded", "runtime-api"]}
        command = gate.target_command({"tools": {"cargo": "/explicit/cargo"}}, target, False)
        self.assertEqual(command[-2:], ["--features", "embedded,runtime-api"])
        self.assertNotIn("--all-features", command)

    def test_sdk_missing_and_partial_execution_fail(self):
        config = {"tools": {"cargo": sys.executable, "python": sys.executable}}
        with self.assertRaises(gate.MissingInput):
            gate.command_plan("sdk", config)
        with self.assertRaises(RuntimeError):
            gate.check_output("sdk", {}, "Python: handshake passed")
        with self.assertRaises(RuntimeError):
            gate.check_output("python", {"python_tests": 0}, "OK")

    def test_classification_rejects_unknown_duplicate_and_wrong_target(self):
        row = {"package": "sandbox", "target": "isolation", "name": "restricted_probe", "stage": "probe"}
        target = {"package": "sandbox", "target": "isolation"}
        self.assertEqual(classify_cases(["restricted_probe"], [row], target)[0]["stage"], "probe")
        for names, rows, owner in ((["unknown"], [row], target), (["restricted_probe"], [row, row], target),
                                   (["restricted_probe"], [row], {**target, "target": "other"}),
                                   (["a::restricted_probe", "b::restricted_probe"], [row], target)):
            with self.assertRaises(RuntimeError):
                classify_cases(names, rows, owner)

    def test_postgres_requires_owned_loopback_case_template_without_override(self):
        def document(url):
            return {"database": {"postgres_test": {"type": "postgres", "url": url}}}
        validate_postgres(document("postgresql://fixture@127.0.0.1:51234/dever_{case}"))
        for url in ("postgresql://fixture@db.example:5432/dever_{case}",
                    "postgresql://fixture@127.0.0.1:5432/fixed", "postgresql://fixture@127.0.0.1/dever_{case}",
                    "postgresql://fixture@127.0.0.1:5432/dever_{case}?dbname=production"):
            with self.assertRaises(gate.MissingInput):
                validate_postgres(document(url))

    def test_failed_command_keeps_exit_and_counts(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with self.assertRaises(RuntimeError):
                gate.run_command([sys.executable, "-c", "print('test result: FAILED. 2 passed; 1 failed; 3 ignored'); raise SystemExit(7)"], directory, 0, 5)
            record = json.loads((directory / "000.command.json").read_text())
            self.assertEqual(record["exit_code"], 7)
            self.assertEqual(record["rust_tests"], {"passed": 2, "failed": 1, "ignored": 3})

    def test_single_harness_counts_parent_instead_of_nested_probe(self):
        child = "test result: ok. 1 passed; 0 failed; 0 ignored"
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for number, (single, parent_passed, parent_ignored, exit_code) in enumerate(
                    ((False, 1, 0, 0), (True, 1, 0, 0), (True, 0, 0, 0),
                     (True, 0, 1, 0), (True, 0, 0, 7))):
                parent_failed = int(exit_code != 0)
                parent = (f"test result: {'FAILED' if parent_failed else 'ok'}. "
                          f"{parent_passed} passed; {parent_failed} failed; {parent_ignored} ignored")
                command = [sys.executable, "-c",
                           f"print({child!r}); print({parent!r}); raise SystemExit({exit_code})"]
                with self.subTest(single=single, parent=parent):
                    if exit_code:
                        with self.assertRaises(RuntimeError):
                            gate.run_command(command, directory, number, 5, single_harness=single)
                    else:
                        gate.run_command(command, directory, number, 5, single_harness=single)
                    record = json.loads((directory / f"{number:03d}.command.json").read_text())
                    self.assertEqual(record["exit_code"], exit_code)
                    self.assertEqual(record["rust_tests"], {
                        "passed": parent_passed + int(not single),
                        "failed": parent_failed, "ignored": parent_ignored})

    def test_exact_ignored_acceptance_requires_one_successful_parent(self):
        target = {"package": "sandbox", "target": "isolation"}
        case = {"name": "parent", "exact": "parent", "stage": "sandbox",
                "target": target, "target_directory": "/unused/target"}
        for counts in ({"passed": 1, "failed": 0, "ignored": 0},
                       {"passed": 0, "failed": 0, "ignored": 0},
                       {"passed": 0, "failed": 0, "ignored": 1},
                       {"passed": 1, "failed": 1, "ignored": 0},
                       {"passed": 2, "failed": 0, "ignored": 0}, None):
            with (self.subTest(counts=counts),
                  patch.object(acceptance, "require_inputs"),
                  patch.object(acceptance, "current_inventory", return_value=[case]),
                  patch.object(acceptance, "target_command", return_value=["cargo"]),
                  patch.object(acceptance, "test_executable", return_value="/unused/test"),
                  patch.object(acceptance, "remove_executables", return_value=[]),
                  patch.object(acceptance, "execute", side_effect=[({}, "build"),
                               ({"rust_tests": counts}, "test")]) as execute):
                result = {"commands": []}
                expected = counts == {"passed": 1, "failed": 0, "ignored": 0}
                with contextlib.nullcontext() if expected else self.assertRaisesRegex(RuntimeError, "one case"):
                    acceptance.ignored_stage("sandbox", {}, {}, Path("/unused"), result)
                self.assertTrue(execute.call_args.kwargs["single_harness"])
                self.assertEqual(len(result.get("cases", [])), int(expected))

    def test_source_and_author_input_fingerprints_detect_real_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in ("Cargo.toml", "Cargo.lock"):
                (root / name).write_text(name)
            (root / "test").mkdir()
            before = gate.source_fingerprint(root)
            (root / "test/new.py").write_text("changed")
            self.assertNotEqual(before, gate.source_fingerprint(root))
            output = root / "generated.pack"
            output.write_text("old")
            config = {"inputs": [str(root)], "author_outputs": [str(output)]}
            author_before = gate.input_fingerprint({}, config, "author")
            native_before = gate.input_fingerprint({}, config, "native")
            output.write_text("new")
            self.assertEqual(author_before, gate.input_fingerprint({}, config, "author"))
            self.assertNotEqual(native_before, gate.input_fingerprint({}, config, "native"))
            (root / "Cargo.lock").write_text("changed immutable input")
            self.assertNotEqual(author_before, gate.input_fingerprint({}, config, "author"))

    def test_cleanup_cannot_delete_author_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "native-acceptance-init"
            executable.write_text("owned author fixture")
            message = json.dumps({"reason": "compiler-artifact", "profile": {"test": True}, "executable": str(executable)})
            with self.assertRaises(RuntimeError):
                gate.remove_executables(message, root, True)
            self.assertTrue(executable.exists())

    def test_failed_workspace_command_still_cleans_its_test_executable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "target/debug/deps/test-owned"
            executable.parent.mkdir(parents=True)
            executable.write_text("rebuildable executable")
            config = {"output": str(root / "results"), "remove_test_executables": True,
                      "tools": {"cargo": sys.executable}}

            def fail_workspace(stage, config, directory, result):
                result["target_directory"] = str(root / "target")
                message = json.dumps({"reason": "compiler-artifact", "profile": {"test": True},
                                      "executable": str(executable)})
                gate.run_command([sys.executable, "-c", f"print({message!r}); raise SystemExit(7)",
                                  "--message-format=json"],
                                 directory, 0, 5)

            with (patch.object(gate, "load_config", return_value=({}, config)),
                  patch.object(gate, "source_fingerprint", return_value="source"),
                  patch.object(gate, "run_workspace", side_effect=fail_workspace),
                  patch.object(acceptance, "validate_budget"),
                  patch.object(sys, "argv", ["run.py", "run", "--stage", "default"]),
                  contextlib.redirect_stdout(io.StringIO())):
                self.assertEqual(gate.main(), 1)
            self.assertFalse(executable.exists())
            record = json.loads(next((root / "results/default").glob("*/result.json")).read_text())
            self.assertEqual(record["status"], "failed")
            self.assertIn("command exited 7", record["error"])
            self.assertEqual(record["removed_test_executables"], [str(executable)])

    def test_missing_input_is_saved_as_blocked_stage_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            config = {"output": temporary}
            with (patch.object(gate, "load_config", return_value=({}, config)),
                  patch.object(gate, "source_fingerprint", return_value="source"),
                  patch.object(gate, "input_fingerprint", side_effect=FileNotFoundError("missing peer")),
                  patch.object(sys, "argv", ["run.py", "run", "--stage", "python"]),
                  contextlib.redirect_stdout(io.StringIO())):
                self.assertEqual(gate.main(), 1)
            record = json.loads(next((Path(temporary) / "python").glob("*/result.json")).read_text())
            self.assertEqual(record["status"], "blocked")
            self.assertIn("missing peer", record["error"])

    def test_inventory_never_reuses_pass_after_a_newer_failure_or_interruption(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            config = {"output": temporary}
            fingerprint = gate.input_fingerprint({}, config, "inventory")
            for index, status in enumerate(("passed", "running")):
                directory = root / "inventory" / str(index)
                directory.mkdir(parents=True)
                (directory / "result.json").write_text(json.dumps({"source": "source", "inputs": fingerprint,
                                                                 "status": status}))
                (directory / "inventory.json").write_text("[]")
            with self.assertRaises(gate.MissingInput):
                acceptance.current_inventory({}, config, {"source": "source"})
            output = io.StringIO()
            with (patch.object(gate, "load_config", return_value=({}, config)),
                  patch.object(gate, "source_fingerprint", return_value="source"),
                  patch.object(gate, "STAGES", ("inventory",)),
                  patch.object(sys, "argv", ["run.py", "summary"]),
                  contextlib.redirect_stdout(output)):
                self.assertEqual(gate.main(), 1)
            self.assertEqual(json.loads(output.getvalue()), {"inventory": "running"})
            (root / "inventory/1/result.json").write_text(json.dumps({
                "source": "source", "inputs": fingerprint, "status": "failed"}))
            with self.assertRaises(gate.MissingInput):
                acceptance.current_inventory({}, config, {"source": "source"})

    def test_inventory_detects_a_missing_owner_even_when_leaf_names_match(self):
        row = {"package": "package", "target": "first", "name": "same_name", "stage": "native"}
        target = {"package": "package", "target": "first", "selector": ["--lib"]}
        artifact = json.dumps({"reason": "compiler-artifact", "profile": {"test": True},
                               "target": {"name": "first"}, "executable": "/owned/test"})
        with (tempfile.TemporaryDirectory() as temporary,
              patch.object(acceptance, "audit_classification", return_value=[row, {**row, "target": "second"}]),
              patch.object(acceptance, "workspace_targets", return_value=([target], Path(temporary))),
              patch.object(acceptance, "execute", side_effect=[({}, artifact), ({}, "same_name: test\n")])):
            with self.assertRaisesRegex(RuntimeError, "second"):
                acceptance.inventory({"tools": {"cargo": "/cargo"}}, Path(temporary), {"commands": []})

    def test_soak_rejects_short_load_errors_drops_missing_budget_and_fd_leak(self):
        samples = [{"rss_bytes": 100, "fds": 3, "elapsed_seconds": index * 600} for index in range(4)]
        case = {"suite": "http2", "implementation": "dever", "transport": "http",
                "budget_after_stop": {"events": {"oom": 0, "oom_kill": 0}, "max": str(64 * 1024 ** 2)},
                "load": {"elapsed_seconds": 1800, "scheduled": 1000, "succeeded": 1000, "errors": 0},
                "disconnected": {"fd_delta": 0}, "raw": samples}
        report = {"status": "complete", "cases": [case, {**case, "transport": "https"}]}
        validate_soak(report, "http2", 1800, 1024, 64)
        for field, value in (("elapsed_seconds", 1799), ("errors", 1), ("succeeded", 989)):
            invalid = json.loads(json.dumps(report))
            invalid["cases"][0]["load"][field] = value
            with self.assertRaises(RuntimeError):
                validate_soak(invalid, "http2", 1800, 1024, 64)
        for field, value in (("transport", "https"), ("implementation", "hyper"),
                             ("disconnected", {"fd_delta": 1}),
                             ("budget_after_stop", {"events": {"oom": 0, "oom_kill": 0}, "max": "max"})):
            invalid = json.loads(json.dumps(report))
            invalid["cases"][0][field] = value
            with self.assertRaises(RuntimeError):
                validate_soak(invalid, "http2", 1800, 1024, 64)
        case["budget_after_stop"] = None
        with self.assertRaises(RuntimeError):
            validate_soak(report, "http2", 1800, 1024, 64)

    def test_cms_soak_checks_descriptor_identity_and_pool_bound_not_total_count(self):
        load = {"elapsed_seconds": 1800, "succeeded": 1000, "errors": 0,
                "fd_delta_after_disconnect": 1,
                "descriptors_before": {"3": "socket:[listener]", "4": "/fixture/platform.db"},
                "descriptors_after": {"3": "socket:[listener]", "4": "/fixture/platform.db",
                                      "5": "/fixture/platform.db"},
                "pooled_descriptor_limits": {"/fixture/platform.db": 4},
                "trend": {"rss_late_growth_bytes": 0}}
        case = {"cgroup": {"events": {"oom": 0, "oom_kill": 0}, "max": str(64 * 1024 ** 2)},
                "concurrent_reads": load}
        report = {"status": "passed", "cases": {"dever": case, "md": case}}
        validate_soak(report, "cms", 1800, 1024, 64)
        for descriptors in ({**load["descriptors_after"], "3": "socket:[replacement]"},
                            {**load["descriptors_after"], "9": "socket:[leaked]"},
                            {**load["descriptors_after"], **{str(n): "/fixture/platform.db" for n in range(10, 15)}}):
            invalid = json.loads(json.dumps(report))
            invalid["cases"]["md"]["concurrent_reads"]["descriptors_after"] = descriptors
            with self.subTest(descriptors=descriptors), self.assertRaises(RuntimeError):
                validate_soak(invalid, "cms", 1800, 1024, 64)


if __name__ == "__main__":
    unittest.main()

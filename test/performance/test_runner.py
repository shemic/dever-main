"""基准运行器的最小正确性检查，不启动网络压测或编译器。"""

import argparse
import errno
import json
import os
from pathlib import Path
from types import SimpleNamespace
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, Mock, patch

import cms
from process import Budget, Process, observe, summarize
from build import (APPLICATION_CONFIGURATION, BENCHMARK_COMMAND, ENTRIES, configuration, dependency_packages, fingerprint,
                   load_postgres_setting, postgres_case_url, postgres_database_case,
                   native_runtime_setting, prepare_cms_project, prepare_native_project,
                   prepare_profile_project, prepare_project,
                   profile_database_setting, stage_application_command)
from run import (HTTP_BODY_BYTES, application_command, fixture_command, async_measurements, csv_positive_ints, http2_summary,
                 cms_cases, cms_summary, expected_orm_checksum, orm_measurements, orm_summary,
                 orm_count, parse_client_cpus, parse_implementations, parser, percentile,
                 positive, report_arguments, required_binaries, require_no_oom, require_no_request_errors,
                 require_successful_requests, server, stage_orm_bundle, transaction_cancel_measurement,
                 validate_cpu_placement, validate_load)
from run import stage_live_bundle, verify_artifacts
from live import lifecycle_seconds, parse_progress, phase_sequence, stable_phase, validate_result
from settings import PEER_CONFIGURATION, configured_peer, stage_peer


class MeasurementTests(unittest.TestCase):
    def test_application_command_matches_generated_declaration(self):
        self.assertEqual(application_command(Path("app")),
                         [Path("app"), BENCHMARK_COMMAND, "{}"])
        with tempfile.TemporaryDirectory(prefix="dever-perf-command-") as temporary:
            module = Path(temporary) / "module"
            stage_application_command(module, "benchmark.item.orm_list")
            self.assertEqual((module / "benchmark/command/api.dever").read_text(),
                             "cmd run = app.run\n")
            self.assertIn("benchmark.item.orm_list()",
                          (module / "benchmark/command/app.dever").read_text())
            self.assertFalse((module / "main.dever").exists())

    def test_generated_projects_use_stock_cms_and_cmd_for_orm_profiles(self):
        settings = configuration(parser().parse_args(["build", "--output", "unused"]))
        with tempfile.TemporaryDirectory(prefix="dever-perf-project-") as temporary:
            root = Path(temporary)
            for source in ("dever", "md"):
                project = root / f"cms-{source}"
                prepare_cms_project(project, source)
                self.assertFalse((project / "module/main.dever").exists())
                self.assertFalse((project / "module/benchmark").exists())
                self.assertEqual((project / "config/setting.json").read_bytes(),
                                 (Path(__file__).parents[2] / "examples/cms" / source
                                  / "config/setting.json").read_bytes())
                self.assertTrue((project / "test/news/article").is_dir())
            orm = root / "orm"
            prepare_project(orm, "orm", settings, "sqlite")
            self.assertFalse((orm / "module/main.dever").exists())
            self.assertTrue((orm / "module/benchmark/command/api.dever").is_file())
            self.assertTrue((orm / "module/benchmark/item/api.dever").is_file())
            for profile in ("base", "sqlite", "postgres", "both"):
                project = root / f"profile-{profile}"
                prepare_profile_project(project, profile_database_setting(profile))
                self.assertFalse((project / "module/main.dever").exists())
                self.assertTrue((project / "module/benchmark/command/api.dever").is_file())

    def test_profile_free_generation_copies_roles_and_only_used_helpers(self):
        settings = configuration(parser().parse_args(["build", "--output", "unused"]))
        expected_config = {
            "runtime": ("lifetime", "idle_sample_ms", "pending", "channel_capacity",
                        "input", "pending_input"),
            "http": ("lifetime", "limits", "http2_limits", "large_body",
                     "certificate", "key"),
            "live": (),
        }
        with tempfile.TemporaryDirectory(prefix="dever-perf-sources-") as temporary:
            for fixture, functions in expected_config.items():
                destination = Path(temporary) / fixture
                prepare_native_project(destination, fixture, settings)
                domain = destination / "module/benchmark" / fixture
                self.assertTrue(all((domain / f"{role}.dever").is_file()
                                    for role in ("app", "port", "adapter", "api")))
                config = destination / "module/benchmark/config/app.dever"
                self.assertEqual(config.is_file(), bool(functions))
                if functions:
                    source = config.read_text()
                    self.assertTrue(all(f"\n{name}()" in source for name in functions))
                    self.assertNotIn("orm_input()", source)
                self.assertEqual((destination / "module/benchmark/bench/app.dever").is_file(),
                                 fixture == "runtime")
                setting = json.loads((destination / "config/setting.json").read_text())
                self.assertEqual(setting, native_runtime_setting(settings) if fixture == "live" else {})
                declarations = (domain / "api.dever").read_text()
                for entry in ENTRIES[fixture]:
                    self.assertIn(f"cmd {entry} = app.{entry}", declarations)
                    self.assertEqual(fixture_command(Path("app"), entry),
                                     [Path("app"), f"benchmark.{fixture}.{entry}", "{}"])
        live = native_runtime_setting(settings)["adapter"]["benchmark.live"]["setting"]
        self.assertEqual(live["lifetime_ms"], settings["lifetime_ms"])
        self.assertEqual(live["limits"]["timeout_ms"], settings["timeout_ms"])
        self.assertNotIn("timeout_ms", live)

    def test_live_adapter_setting_is_private_to_each_executable(self):
        settings = {"lifetime_ms": 4000, "timeout_ms": 2000, "connections": 8}
        with tempfile.TemporaryDirectory(prefix="dever-live-bundle-") as temporary:
            root = Path(temporary)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            for protocol in ("tcp", "ws"):
                (artifacts / protocol).write_bytes(protocol.encode())
            output = root / "output"
            output.mkdir()
            args = SimpleNamespace(artifacts=artifacts, output=output)
            for protocol in ("tcp", "ws"):
                executable = stage_live_bundle(args, settings, f"live-{protocol}-0", protocol)
                self.assertEqual(executable.read_bytes(), protocol.encode())
                setting = executable.parent / "config/setting.json"
                self.assertEqual(json.loads(setting.read_text()), native_runtime_setting(settings))
                self.assertEqual(setting.stat().st_mode & 0o777, 0o600)
            self.assertFalse((artifacts / "config").exists())

    def test_cms_cases_use_stock_config_and_publish_result(self):
        with tempfile.TemporaryDirectory(prefix="dever-perf-cms-case-") as temporary:
            root = Path(temporary)
            artifacts = root / "artifacts"
            output = root / "output"
            artifacts.mkdir()
            output.mkdir()
            for source, name in (("dever", "cms"), ("md", "cms-md")):
                (artifacts / name).write_bytes(source.encode())
                config = artifacts / "source" / f"cms-{source}" / "config"
                config.mkdir(parents=True)
                (config / "setting.json").write_text("{}")
            args = SimpleNamespace(artifacts=artifacts, output=output, repeats=1,
                                   _cms_sources=("dever", "md"), cgroup_parent=None,
                                   memory_mib=None, idle_seconds=0.1, interval=0.01,
                                   server_cpu=None, cpu_quota=None)
            result = {"contract": {"articles": 16, "titles": [f"Article {i}" for i in range(16)],
                                   "duplicate_publish_status": 409},
                      "ready_seconds": 0.2, "workload_seconds": 0.5,
                      "published_articles_per_second": 32,
                      "latency": {}, "resources": {"rss_bytes_median": 1,
                                                     "pss_bytes_median": 1,
                                                     "rss_high_water_bytes": 2},
                      "raw": [], "budget_before_stop": None, "cgroup": None}
            with patch("cms.run_case", return_value=result) as run_case:
                cases = cms_cases(args, {}, {})
            self.assertEqual(len(cases), 2)
            self.assertEqual(run_case.call_count, 2)
            for call, source in zip(run_case.call_args_list, ("dever", "md")):
                identity = call.args[0]
                self.assertEqual(identity["config"], str(
                    artifacts / "source" / f"cms-{source}" / "config/setting.json"))
                self.assertEqual(identity["fingerprint"], fingerprint(Path(identity["executable"])))
                self.assertEqual(call.kwargs["articles"], 16)
            self.assertEqual(cms_summary(cases)["dever"]["articles"], 16)
            self.assertTrue(all(case["workload_mode"] == "sequential_publish" for case in cases))
            self.assertTrue(all("ready_value" not in case for case in cases))

    def test_focused_cms_build_selection_keeps_the_full_build_as_default(self):
        self.assertEqual(parser().parse_args(["build", "--output", "unused"]).selection, "all")
        self.assertEqual(parser().parse_args(
            ["build", "--output", "unused", "--selection", "cms-profiles"]
        ).selection, "cms-profiles")
        self.assertEqual(parser().parse_args(
            ["build", "--output", "unused", "--selection", "profiles"]
        ).selection, "profiles")

    def test_cms_descriptor_recovery_bounds_pool_and_preserves_other_resources(self):
        before = {"0": "/dev/null", "3": "socket:[listener]", "4": "/fixture/platform.db"}
        after = {**before, "5": "/fixture/platform.db", "6": "/fixture/platform.db-wal"}
        load = {"descriptors_before": before, "descriptors_after": after,
                "pooled_descriptor_limits": {"/fixture/platform.db": 4,
                                             "/fixture/platform.db-wal": 4,
                                             "/fixture/platform.db-shm": 1}}
        cms.validate_read_descriptor_recovery(load)
        invalid = [
            {**after, "9": "socket:[request]"},
            {**after, "9": "socket:[listener]"},
            {**after, "3": "socket:[replacement]"},
            {**after, "0": "/unexpected/file"},
            {**after, **{str(index): "/fixture/platform.db" for index in range(10, 15)}},
            {**after, **{str(index): "/fixture/platform.db-wal" for index in range(10, 15)}},
            {**after, "10": "/fixture/platform.db-shm", "11": "/fixture/platform.db-shm"},
        ]
        for descriptors in invalid:
            with self.subTest(descriptors=descriptors), self.assertRaises(RuntimeError):
                cms.validate_read_descriptor_recovery({**load, "descriptors_after": descriptors})

    def test_cms_failed_recovery_keeps_case_and_aggregate_evidence(self):
        with tempfile.TemporaryDirectory(prefix="dever-cms-failed-report-") as temporary:
            root = Path(temporary)
            executable = root / "app"
            executable.write_bytes(b"fixture")
            config = root / "setting.json"
            config.write_text("{}")
            identity = {"executable": str(executable), "fingerprint": fingerprint(executable),
                        "config": str(config)}
            manifest = {"format": "dever-cms-publish-v1", "cases": {"dever": identity, "md": identity}}
            client = Mock(cookies={}, timings={})
            client.request.return_value = []
            snapshots = iter(({"3": "socket:[listener]"},
                              {"3": "socket:[listener]", "4": "socket:[leaked]"}))

            def descriptors():
                self.assertTrue(client.close.called, "control client must close before the baseline")
                return next(snapshots)

            process = Mock(samples=[{"fds": 1}], descriptors=descriptors)
            context = MagicMock()
            context.__enter__.return_value = process
            context.__exit__.return_value = False
            budget = Mock(final={"events": {"oom": 0, "oom_kill": 0}})
            settings = {"database": {"default": {"type": "sqlite", "path": "data/platform.db",
                                                   "tenant_directory": "data/tenants", "max_connections": 4}}}
            load = {"elapsed_seconds": 75, "succeeded": 100, "errors": 0}
            with (patch("cms.isolated_settings", return_value=settings),
                  patch("cms.provision_identity", return_value=("actor", "password")),
                  patch("cms.CmsClient", return_value=client), patch("cms.Budget", return_value=budget),
                  patch("cms.Process", return_value=context), patch("cms.wait_ready", return_value=0.1),
                  patch("cms.publish_workload", return_value={"articles": 1}), patch("cms.observe"),
                  patch("cms.concurrent_reads", return_value=load) as reads):
                with self.assertRaisesRegex(RuntimeError, "read descriptors did not recover"):
                    cms.run_cases(manifest, root / "output", articles=1, cgroup_parent=None,
                                  memory_mib=None, concurrency=8, read_seconds=75)
                snapshots = iter(({"3": "socket:[listener]"},))
                reads.side_effect = RuntimeError("read fixture failed")
                with self.assertRaisesRegex(RuntimeError, "read fixture failed"):
                    cms.run_cases(manifest, root / "read-failure", articles=1, cgroup_parent=None,
                                  memory_mib=None, concurrency=8, read_seconds=75)
            case = json.loads((root / "output/dever/report.json").read_text())
            report = json.loads((root / "output/report.json").read_text())
            self.assertEqual(case["status"], "failed")
            self.assertEqual(case["raw"], [{"fds": 1}])
            self.assertEqual(case["concurrent_reads"]["succeeded"], 100)
            self.assertEqual(case["concurrent_reads"]["descriptors_after"]["4"], "socket:[leaked]")
            self.assertEqual(case["cgroup"], budget.final)
            self.assertEqual(report["cases"], {"dever": case})
            self.assertEqual(report["status"], "failed")
            early = json.loads((root / "read-failure/report.json").read_text())["cases"]["dever"]
            self.assertEqual(early["contract"], {"articles": 1})
            self.assertEqual(early["concurrent_reads"]["descriptors_before"], {"3": "socket:[listener]"})
            self.assertNotIn("succeeded", early["concurrent_reads"])
            self.assertEqual(context.__exit__.call_count, 2)
            self.assertEqual(budget.close.call_count, 2)

    def test_cms_requires_every_declared_source_binary(self):
        args = SimpleNamespace(suite="cms", _postgres_setting=None,
                               _cms_sources=("dever", "md"))
        self.assertEqual(required_binaries(args), {"cms", "cms-md"})
        args._cms_sources = ("dever",)
        self.assertEqual(required_binaries(args), {"cms"})

    def test_peer_staging_preserves_binary_and_isolates_settings(self):
        with tempfile.TemporaryDirectory(prefix="dever-peer-setting-") as temporary:
            root = Path(temporary)
            source = root / "peer"
            source.write_bytes(b"owned executable")
            source.chmod(0o700)
            for workers in (1, 4):
                directory = root / str(workers)
                directory.mkdir()
                executable = stage_peer(source, directory, {"workers": workers})
                self.assertEqual(executable.read_bytes(), source.read_bytes())
                self.assertTrue(executable.samefile(source))
                self.assertEqual(executable.stat().st_mode & 0o777, 0o700)
                setting = directory / "config/setting.json"
                self.assertEqual(json.loads(setting.read_text()), {"benchmark": {"workers": workers}})
                self.assertEqual(setting.stat().st_mode & 0o777, 0o600)
                with self.assertRaises(FileExistsError):
                    stage_peer(source, directory, {"workers": 8})
            self.assertFalse((root / "config").exists())

    def test_peer_staging_copies_only_across_filesystems(self):
        with tempfile.TemporaryDirectory(prefix="dever-peer-filesystem-") as temporary:
            root = Path(temporary)
            source = root / "peer"
            source.write_bytes(b"owned executable")
            source.chmod(0o700)
            directory = root / "cross-filesystem"
            directory.mkdir()
            with patch("settings.os.link", side_effect=OSError(errno.EXDEV, "different device")):
                executable = stage_peer(source, directory, {})
            self.assertEqual(executable.read_bytes(), source.read_bytes())
            self.assertEqual(executable.stat().st_mode & 0o777, 0o700)
            self.assertFalse(executable.samefile(source))
            denied = root / "denied"
            denied.mkdir()
            with patch("settings.os.link", side_effect=PermissionError(errno.EACCES, "denied")):
                with self.assertRaises(PermissionError):
                    stage_peer(source, denied, {})
            self.assertFalse((denied / "peer").exists())

    def test_peer_paths_come_from_project_setting(self):
        with tempfile.TemporaryDirectory(prefix="dever-peer-path-") as temporary:
            root = Path(temporary)
            (root / "config").mkdir()
            (root / "peer").write_bytes(b"binary")
            setting = root / "config/setting.json"
            setting.write_text(json.dumps({"performance": {"network_peer": "peer"}}))
            self.assertEqual(configured_peer("network_peer", setting), root / "peer")
            for value in ({}, {"performance": []}, {"performance": {"network_peer": 123}}):
                setting.write_text(json.dumps(value))
                with self.assertRaisesRegex(ValueError, "performance.network_peer"):
                    configured_peer("network_peer", setting)

    def test_process_stages_peer_before_starting_child(self):
        with tempfile.TemporaryDirectory(prefix="dever-peer-process-") as temporary:
            root = Path(temporary)
            command = [sys.executable, "-c", (
                "import json, pathlib, sys; "
                "print((pathlib.Path(sys.executable).parent / 'config/setting.json').read_text())"
            )]
            with Process(command, root / "child", peer_settings={"workers": 3}) as process:
                self.assertEqual(process.child.wait(timeout=5), 0)
                process.check()
                self.assertEqual(json.loads(process.text()), {"benchmark": {"workers": 3}})

    @staticmethod
    def postgres_setting():
        return {
            "database": {
                "postgres_test": {
                    "type": "postgres",
                    "url": "postgres://user:secret@localhost/dever_{case}",
                    "tls": "disabled",
                    "max_connections": 4,
                }
            }
        }

    def test_orm_samples_are_bounded_and_report_percentiles(self):
        result = orm_measurements(
            "SAMPLE|crud|1|1000000|2\nSAMPLE|crud|1|3000000|2\n",
            "crud", 2, 2, 0.5,
        )
        self.assertEqual(result["operations"], 2)
        self.assertEqual(result["throughput_operations_per_second"], 4)
        self.assertEqual(result["latency_ms"], {"p50": 1, "p95": 3, "p99": 3})
        with self.assertRaises(ValueError):
            orm_measurements("SAMPLE|crud|1|1|2\nSAMPLE|crud|1|1|2\n", "crud", 1, 2, 1)
        with self.assertRaises(ValueError):
            orm_measurements("SAMPLE|list|1|1|2\n", "crud", 1, 2, 1)
        with self.assertRaises(ValueError):
            orm_measurements("SAMPLE|crud|1|1|1\n", "crud", 1, 2, 1)

    def test_orm_checksums_cover_empty_cancel_and_bounded_pages(self):
        settings = {"orm_rows": 20}
        self.assertEqual(expected_orm_checksum("crud", settings), 2)
        self.assertEqual(expected_orm_checksum("list", settings), 40)
        self.assertEqual(expected_orm_checksum("cursor", settings), 20)
        self.assertEqual(expected_orm_checksum("stream", settings), 20)
        self.assertEqual(expected_orm_checksum("pool", settings), 20)
        result = transaction_cancel_measurement(0, 0.01)
        self.assertEqual(result["raw"][0]["checksum"], 0)
        with self.assertRaises(RuntimeError):
            transaction_cancel_measurement(1, 0.01)

    def test_postgres_cases_require_explicit_isolation_template(self):
        setting = self.postgres_setting()
        self.assertEqual(
            postgres_case_url(setting, "orm_postgres_crud_0"),
            "postgres://user:secret@localhost/dever_orm_postgres_crud_0",
        )
        for value in ("postgres://localhost/shared", "sqlite:///{case}",
                      "postgres://localhost/shared?application_name={case}"):
            setting["database"]["postgres_test"]["url"] = value
            with self.assertRaises(ValueError):
                postgres_case_url(setting, "case")

    def test_postgres_setting_is_loaded_from_the_project_config(self):
        with tempfile.TemporaryDirectory(prefix="dever-setting-") as directory:
            path = Path(directory) / "setting.json"
            path.write_text(json.dumps(self.postgres_setting()))
            self.assertEqual(load_postgres_setting(path), self.postgres_setting())
            path.write_text(json.dumps({
                "database": {
                    "default": {
                        "type": "sqlite",
                        "path": "data/db/dever.db",
                        "max_connections": 1,
                        "max_page_size": 100,
                    }
                }
            }))
            self.assertIsNone(load_postgres_setting(path))
            path.write_text(json.dumps({
                "database": {"postgres_test": {"type": "sqlite"}}
            }))
            with self.assertRaises(ValueError):
                load_postgres_setting(path)
            self.assertIsNone(load_postgres_setting(path.with_name("missing.json")))

    def test_profile_settings_cover_each_database_capability(self):
        base = profile_database_setting("base")
        sqlite = profile_database_setting("sqlite")
        postgres = profile_database_setting("postgres")
        both = profile_database_setting("both")
        self.assertEqual(base, {})
        self.assertEqual(tuple(sqlite["database"]), ("default",))
        self.assertEqual(sqlite["database"]["default"]["type"], "sqlite")
        self.assertEqual(postgres["database"]["default"]["type"], "postgres")
        self.assertEqual(postgres["database"]["default"]["url"],
                         "postgres://build-only.invalid/dever_profile")
        self.assertEqual(tuple(both["database"]), ("default", "postgres_profile"))
        self.assertEqual(both["database"]["postgres_profile"]["type"], "postgres")
        self.assertNotIn("postgres_test", both["database"])

    def test_postgres_cancel_fixture_binds_int_as_bigint(self):
        fixture = (Path(__file__).parent / "fixtures/orm/app/model/item.dever").read_text()

        self.assertIn("generate_series(1::bigint, $1::bigint)", fixture)

    def test_dependency_report_is_unique_and_stable(self):
        self.assertEqual(
            dependency_packages("z v1\na v1\nz v1 (*)\n"),
            ["a v1", "z v1"],
        )

    def test_report_omits_private_postgres_configuration(self):
        arguments = report_arguments(SimpleNamespace(
            _postgres_setting=self.postgres_setting(),
            artifacts=Path("artifacts"),
            client_workers=4,
            client_cpus=(1, 2, 3, 4),
        ))
        self.assertNotIn("_postgres_setting", arguments)
        self.assertEqual(arguments["artifacts"], "artifacts")
        self.assertEqual(json.loads(json.dumps(arguments))["client_cpus"], [1, 2, 3, 4])
        self.assertEqual(arguments["client_workers"], 4)

    def test_orm_bundle_uses_a_fresh_external_setting_per_case(self):
        settings = {"orm_sqlite_connections": 1, "orm_postgres_connections": 4,
                    "orm_rows": 128}
        with tempfile.TemporaryDirectory(prefix="dever-orm-bundle-") as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            (artifacts / "sqlite-crud").write_bytes(b"binary")
            output = root / "output"
            output.mkdir()
            args = SimpleNamespace(output=output, artifacts=artifacts, _postgres_setting=None)
            first = stage_orm_bundle(args, settings, "sqlite", "crud", 0)
            second = stage_orm_bundle(args, settings, "sqlite", "crud", 1)
            self.assertNotEqual(first[1].parent, second[1].parent)
            first_setting = json.loads((first[1].parent / "config/setting.json").read_text())
            second_setting = json.loads((second[1].parent / "config/setting.json").read_text())
            self.assertEqual(first_setting["database"], second_setting["database"])
            self.assertNotEqual(first_setting["auth"], second_setting["auth"])
            self.assertEqual(first_setting["database"]["default"]["path"], "data/db/orm.db")
            self.assertEqual((first[1].parent / "config/setting.json").stat().st_mode & 0o777,
                             0o600)

    def test_orm_http_bundles_use_isolated_ports_and_private_auth(self):
        settings = {"orm_sqlite_connections": 1, "orm_rows": 17}
        with tempfile.TemporaryDirectory(prefix="dever-orm-http-bundle-") as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            (artifacts / "sqlite-http").write_bytes(b"binary")
            output = root / "output"
            output.mkdir()
            args = SimpleNamespace(output=output, artifacts=artifacts, _postgres_setting=None)
            bundles = [stage_orm_bundle(args, settings, "sqlite", entry, 0,
                                        binary_entry="http" if entry == "cancel" else None)
                       for entry in ("http", "cancel")]
            documents = [json.loads((bundle[1].parent / "config/setting.json").read_text())
                         for bundle in bundles]
            self.assertTrue(all(0 < document["http"]["port"] < 65536
                                for document in documents))
            self.assertTrue(all(document["http"]["host"] == "127.0.0.1"
                                for document in documents))
            self.assertNotEqual(documents[0]["auth"], documents[1]["auth"])
            self.assertTrue(all(document["sites"]["benchmark"]["path"] == ""
                                for document in documents))
            self.assertTrue(all((bundle[1].parent / "config/setting.json").stat().st_mode & 0o777
                                == 0o600 for bundle in bundles))

    def test_orm_count_requires_the_current_api_envelope(self):
        with patch("run.http.client.HTTPConnection") as connection:
            response = connection.return_value.getresponse.return_value
            response.status = 200
            response.getheader.return_value = "application/json; charset=utf-8"
            response.read.return_value = b'{"code":0,"message":"ok","data":17}'
            self.assertEqual(orm_count(12345, 0.2), 17)
            connection.return_value.request.assert_called_with(
                "GET", "/benchmark/item/count", headers={"Connection": "close"}
            )
            for invalid in (b'{"ok":true}', b'{"code":0,"message":"ok","data":true}',
                            b'{"code":0,"message":"ok","data":-1}'):
                response.read.return_value = invalid
                with self.assertRaises(ValueError):
                    orm_count(12345, 0.2)

    def test_postgres_bundle_uses_the_named_project_setting(self):
        settings = {"orm_sqlite_connections": 1, "orm_postgres_connections": 4,
                    "orm_rows": 128}
        with tempfile.TemporaryDirectory(prefix="dever-postgres-bundle-") as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            (artifacts / "postgres-crud").write_bytes(b"binary")
            output = root / "output"
            output.mkdir()
            args = SimpleNamespace(
                output=output,
                artifacts=artifacts,
                _postgres_setting=self.postgres_setting(),
            )
            _, executable, _, _ = stage_orm_bundle(args, settings, "postgres", "crud", 2)
            setting_path = executable.parent / "config/setting.json"
            setting = json.loads(setting_path.read_text())
            self.assertEqual(tuple(setting["database"]), ("default",))
            case = postgres_database_case(output, "orm-postgres-crud-2")
            self.assertEqual(
                setting["database"]["default"]["url"],
                f"postgres://user:secret@localhost/dever_{case}",
            )
            self.assertEqual(setting_path.stat().st_mode & 0o777, 0o600)

    def test_postgres_database_names_isolate_separate_reports(self):
        first = postgres_database_case(Path("first"), "orm-postgres-crud-0")
        second = postgres_database_case(Path("second"), "orm-postgres-crud-0")
        self.assertNotEqual(first, second)
        self.assertTrue(first.endswith("_orm_postgres_crud_0"))

    def test_artifact_verification_rejects_missing_and_changed_files(self):
        with tempfile.TemporaryDirectory(prefix="dever-artifacts-") as directory:
            root = Path(directory)
            binary = root / "app"
            certificate = root / "tls/root.pem"
            certificate.parent.mkdir()
            binary.write_bytes(b"binary")
            certificate.write_bytes(b"certificate")
            manifest = {
                "peer_configuration": PEER_CONFIGURATION,
                "application_configuration": APPLICATION_CONFIGURATION,
                "binaries": {"app": fingerprint(binary)},
                "files": {"tls/root.pem": fingerprint(certificate)},
            }
            verify_artifacts(root, manifest, {"app"})
            with self.assertRaisesRegex(ValueError, "obsolete configuration contract"):
                verify_artifacts(root, {**manifest, "peer_configuration": "old"}, {"app"})
            with self.assertRaisesRegex(ValueError, "obsolete application contract"):
                verify_artifacts(root, {**manifest, "application_configuration": "rust-native"}, {"app"})
            with self.assertRaises(ValueError):
                verify_artifacts(root, manifest, {"app", "missing"})
            certificate.write_bytes(b"changed")
            with self.assertRaises(ValueError):
                verify_artifacts(root, manifest, {"app"})

    def test_server_cleanup_does_not_mask_primary_failure(self):
        args = SimpleNamespace(cgroup_parent=None, memory_mib=None, cpu_quota=None,
                               server_cpu=None)
        with tempfile.TemporaryDirectory(prefix="dever-server-cleanup-") as directory:
            with patch("run.Budget") as budget_type, patch("run.Process"):
                budget_type.return_value.close.side_effect = OSError("cleanup")
                with self.assertRaisesRegex(ValueError, "primary") as caught:
                    with server(args, ["unused"], Path(directory) / "case", {}):
                        raise ValueError("primary")
                self.assertTrue(any("cannot close benchmark budget" in note
                                    for note in caught.exception.__notes__))

    def test_orm_summary_keeps_driver_and_operation_separate(self):
        def case(driver, entry, latency):
            return {"suite": "orm", "driver": driver, "entry": entry,
                    "binary": {"bytes": 10},
                    "resources": {"rss_bytes_median": 20, "pss_bytes_median": 15,
                                  "rss_high_water_bytes": 25},
                    "measurement": {"throughput_operations_per_second": 100,
                                    "latency_ms": {"p50": latency, "p95": latency + 1,
                                                   "p99": latency + 2}}}

        summary = orm_summary([case("sqlite", "crud", 1), case("postgres", "crud", 2)])
        self.assertEqual([(value["driver"], value["entry"]) for value in summary],
                         [("postgres", "crud"), ("sqlite", "crud")])
        self.assertEqual(summary[1]["latency_p99_ms_median"], 3)

    def test_cms_summary_reports_business_baseline(self):
        cases = [{
            "suite": "cms",
            "source": "dever",
            "binary": {"bytes": 30},
            "ready_seconds": 0.5,
            "contract": {"articles": 16},
            "workload_seconds": 0.25,
            "published_articles_per_second": 64,
            "resources": {"rss_bytes_median": 20, "pss_bytes_median": 15,
                          "rss_high_water_bytes": 25},
            "cgroup": {"peak_bytes": 40},
        }]
        self.assertEqual(
            cms_summary(cases),
            {"dever": {"repeats": 1, "workload_mode": "sequential_publish",
                       "binary_bytes": 30, "articles": 16,
                       "ready_seconds_median": 0.5, "workload_seconds_median": 0.25,
                       "published_articles_per_second_median": 64,
                       "rss_bytes_median": 20,
                       "pss_bytes_median": 15, "rss_high_water_bytes_median": 25,
                       "cgroup_peak_bytes_median": 40}},
        )

    def test_cms_summary_separates_source_variants(self):
        cases = [
            {"suite": "cms", "source": source, "binary": {"bytes": size},
             "ready_seconds": 0.5, "contract": {"articles": 16},
             "workload_seconds": 0.25, "published_articles_per_second": 64,
             "resources": {"rss_bytes_median": 20, "pss_bytes_median": 15,
                           "rss_high_water_bytes": 25}, "cgroup": None}
            for source, size in (("dever", 30), ("md", 31))
        ]
        summary = cms_summary(cases)
        self.assertEqual(summary["dever"]["binary_bytes"], 30)
        self.assertEqual(summary["md"]["binary_bytes"], 31)

    def test_live_lifecycle_includes_all_three_phase_deadlines(self):
        self.assertAlmostEqual(lifecycle_seconds(2, 0.3, 2000), 23.2)

    def test_live_phase_order_and_partial_writes(self):
        text = "PHASE|0|connecting\nPHASE|0|connected\nPHASE|0|clos"
        self.assertEqual(parse_progress(text, 1), (phase_sequence(1)[:2], None))
        with self.assertRaises(ValueError):
            parse_progress("PHASE|0|connected\n", 1)
        with self.assertRaises(ValueError):
            parse_progress("PHASE|0|connecting\nPHASE|0|connecting\n", 1)

    def test_live_sample_is_only_attributed_to_a_stable_phase(self):
        connecting = phase_sequence(1)[:1]
        connected = phase_sequence(1)[:2]
        self.assertEqual(stable_phase(connected, connected), (0, "connected"))
        self.assertIsNone(stable_phase(connecting, connected))
        self.assertIsNone(stable_phase([], connecting))

    def test_live_incomplete_cleanup_cannot_pass(self):
        result = {"protocol": "tcp", "connections": 4, "cycles": 3,
                  "attempted_connections": 12, "connected": 12, "disconnected": 12,
                  "messages": 24, "errors": 0, "elapsed_ms": 3000}
        validate_result(result, "tcp", 4, 3)
        for key, value in (("disconnected", 11), ("errors", 1), ("messages", 0)):
            with self.assertRaises(ValueError):
                validate_result({**result, key: value}, "tcp", 4, 3)

    def test_missing_cgroup_controller_still_removes_owned_empty_group(self):
        with tempfile.TemporaryDirectory(prefix="dever-perf-test-") as directory:
            budget = Budget(Path(directory), None, None)
            owned = budget.path
            with self.assertRaises(FileNotFoundError):
                budget.close()
            self.assertFalse(owned.exists())

    def test_dropped_and_failed_requests_cannot_disappear_from_report(self):
        result = {"scheduled": 10, "sent": 4, "completed": 4, "succeeded": 0,
                  "errors": 4, "dropped": 6, "dropped_capacity": 5, "dispatch_expired": 1,
                  "latency_samples": 0, "request_latency_samples": 0, "dispatch_lag_samples": 0,
                  "latency_ms": None, "request_latency_ms": None, "dispatch_lag_ms": None,
                  "elapsed_seconds": 1, "actual_success_qps": 0, "http_version": "h2",
                  "physical_connections": 1, "streams_per_connection": 16,
                  "request_slots": 16, "successful_handshakes": 1}
        validate_load(result)
        with self.assertRaises(ValueError):
            validate_load({**result, "errors": 3})
        with self.assertRaises(ValueError):
            validate_load({**result, "latency_ms": {"p50": 0, "p95": 0, "p99": 0}})
        with self.assertRaises(ValueError):
            validate_load({**result, "dispatch_lag_samples": 1,
                           "dispatch_lag_ms": {"sample_count": 2, "p50": 0,
                                               "p95": 0, "p99": 0}})
        with self.assertRaises(ValueError):
            validate_load({**result, "request_slots": 15})
        with self.assertRaises(ValueError):
            validate_load({**result, "successful_handshakes": 2})

    def test_http2_run_fails_on_request_errors_or_oom(self):
        require_no_request_errors({"errors": 0}, "load")
        with self.assertRaises(RuntimeError):
            require_no_request_errors({"errors": 1}, "load")
        require_no_oom(None)
        require_no_oom({"events": {"oom": 0, "oom_kill": 0, "oom_group_kill": 0}})
        with self.assertRaises(RuntimeError):
            require_no_oom({"events": {"oom": 1, "oom_kill": 0, "oom_group_kill": 0}})

    def test_orm_http_requires_at_least_one_success_without_a_qps_threshold(self):
        require_successful_requests({"succeeded": 1}, "ORM HTTP load")
        with self.assertRaises(RuntimeError):
            require_successful_requests({"succeeded": 0}, "ORM HTTP load")

    def test_http2_summary_keeps_topologies_separate(self):
        def case(connections, repeat, qps, p99, rss):
            return {"suite": "http2", "implementation": "dever", "transport": "http",
                    "path": "/plain", "rate": 5000.0, "physical_connections": connections,
                    "streams_per_connection": 16, "repeat": repeat,
                    "load": {"actual_success_qps": qps, "dropped": 0,
                             "request_latency_ms": {"p99": p99}},
                    "resources": {"rss_bytes_median": rss, "pss_bytes_median": rss - 10,
                                  "rss_high_water_bytes": rss + 10}}

        summary = http2_summary([case(1, 0, 90, 2, 100), case(1, 1, 110, 4, 120),
                                 case(4, 0, 200, 1, 140)])
        self.assertEqual(len(summary), 2)
        self.assertEqual(summary[0]["physical_connections"], 1)
        self.assertEqual(summary[0]["actual_success_qps_median"], 100)
        self.assertEqual(summary[0]["request_latency_p99_ms_median"], 3)
        self.assertEqual(summary[0]["server_rss_bytes_median"], 110)
        self.assertEqual(summary[1]["physical_connections"], 4)

    def test_warmup_and_checksum_are_checked_before_aggregation(self):
        lines = ["SAMPLE|tasks|4|400000|480"]
        lines += [f"SAMPLE|tasks|4|{nanos}|480" for nanos in range(40, 76, 4)]
        result = async_measurements("\n".join(lines), {"iterations": 4, "pending": 2})
        self.assertEqual(result["median_ns_per_operation"], 14)
        self.assertEqual(len(result["raw"]), 10)
        with self.assertRaises(ValueError):
            async_measurements("\n".join(lines).replace("|480", "|479"), {"iterations": 4})
        with self.assertRaises(ValueError):
            async_measurements("\n".join(lines[:-1]), {"iterations": 4})

    def test_cancel_reports_full_group_drain_and_has_no_fake_warmup(self):
        result = async_measurements("SAMPLE|cancel|32|32000|32", {"pending": 32})
        self.assertFalse(result["warmup_samples"])
        self.assertEqual(result["median_ns_per_operation"], 1000)

    def test_percentile_uses_nearest_rank(self):
        self.assertEqual(percentile([9, 2, 5, 1], 0.5), 2)
        self.assertEqual(percentile([9, 2, 5, 1], 0.99), 9)
        with self.assertRaises(argparse.ArgumentTypeError):
            positive("nan")

    def test_http2_matrix_lists_reject_empty_duplicate_and_unknown_values(self):
        self.assertEqual(csv_positive_ints("1,4", "connections"), (1, 4))
        self.assertEqual(parse_implementations("dever,hyper"), ("dever", "hyper"))
        for value in ("", "1,", "1,1"):
            with self.assertRaises(ValueError):
                csv_positive_ints(value, "connections")
        for value in ("", "dever,", "dever,dever", "dever,other"):
            with self.assertRaises(ValueError):
                parse_implementations(value)

    def test_client_cpu_configuration_rejects_invalid_sets_and_placement(self):
        self.assertEqual(parse_client_cpus("0,1,2"), (0, 1, 2))
        for value in ("", "1,", "1,,2", "1,1", "-1", "other"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse_client_cpus(value)
        args = parser().parse_args(["run", "--artifacts", "artifacts", "--output", "output",
                                    "--client-workers", "4", "--client-cpus", "1,2,3,4"])
        self.assertEqual((args.client_workers, args.client_cpus), (4, (1, 2, 3, 4)))
        with self.assertRaises(ValueError):
            validate_cpu_placement(0, (1, 5), {0, 1, 2, 3, 4})
        with self.assertRaises(ValueError):
            validate_cpu_placement(0, (0, 1), {0, 1, 2, 3, 4})
        with self.assertRaises(ValueError):
            validate_cpu_placement(-1, (1,), {0, 1})
        validate_cpu_placement(0, (1, 2), {0, 1, 2})

    def test_process_affinity_is_applied_only_to_owned_child(self):
        allowed = os.sched_getaffinity(0)
        cpus = tuple(sorted(allowed)[:2])
        with tempfile.TemporaryDirectory(prefix="dever-perf-test-") as directory:
            command = [sys.executable, "-c",
                       "import os; print(sorted(os.sched_getaffinity(0)), flush=True)"]
            with Process(command, Path(directory) / "affinity", cpus=cpus) as process:
                self.assertEqual(process.child.wait(timeout=5), 0)
                process.check()
                self.assertEqual(json.loads(process.text()), list(cpus))
        self.assertEqual(os.sched_getaffinity(0), allowed)

    def test_http_body_size_metadata_matches_fixtures(self):
        self.assertEqual(HTTP_BODY_BYTES, {"/plain": 5, "/json": 11, "/bytes": 65_536})

    def test_live_process_sampling_and_owned_cleanup(self):
        with tempfile.TemporaryDirectory(prefix="dever-perf-test-") as directory:
            command = [sys.executable, "-c", "import time; print('READY|0', flush=True); time.sleep(10)"]
            with Process(command, Path(directory) / "process") as process:
                self.assertEqual(process.ready(), 0)
                observe([process], 0.04, 0.01)
                result = summarize(process.samples)
                self.assertGreater(result["rss_bytes_median"], 0)
                self.assertGreater(result["pss_bytes_median"], 0)
                self.assertGreaterEqual(result["threads_median"], 1)
                self.assertGreaterEqual(result["fds_median"], 3)
            self.assertIsNotNone(process.child.poll())

    def test_early_exit_does_not_become_successful_readiness(self):
        with tempfile.TemporaryDirectory(prefix="dever-perf-test-") as directory:
            with Process([sys.executable, "-c", "raise SystemExit(7)"], Path(directory) / "failed") as process:
                with self.assertRaises(RuntimeError):
                    process.ready()

    def test_memory_teardown_can_precede_waitpid_exit(self):
        with tempfile.TemporaryDirectory(prefix="dever-perf-test-") as directory:
            command = [sys.executable, "-c", "import time; print('READY|0', flush=True); time.sleep(10)"]
            with Process(command, Path(directory) / "teardown") as process:
                process.ready()
                count = len(process.samples)
                with patch("process.process_sample", side_effect=ProcessLookupError):
                    process.sample()
                self.assertEqual(len(process.samples), count)
                self.assertIsNone(process.child.poll())


if __name__ == "__main__":
    unittest.main()

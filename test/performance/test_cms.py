import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from cms import (CmsClient, ReadLatency, concurrent_reads, decode_response, execute, isolated_settings, latency_summary,
                 publish_workload, run_postgres_case, run_postgres_rbac)
from settings import write_private_json


class CmsMeasurementTests(unittest.TestCase):
    def test_long_read_histogram_is_bounded_and_reports_upper_bounds(self):
        latency = ReadLatency()
        for _ in range(10000):
            latency.add(3)
        other = ReadLatency()
        other.add(100)
        latency.merge(other)
        self.assertEqual(len(latency.buckets), 2048)
        self.assertEqual(latency.count, 10001)
        self.assertGreaterEqual(latency.percentile(.99), 3)
        self.assertLess(latency.percentile(.99), 3.05)
        client = CmsClient(12345)
        client.record_timings = False
        client.connection = SimpleNamespace(request=Mock(), getresponse=Mock(return_value=SimpleNamespace(
            status=200, read=Mock(return_value=b'{"code":0,"message":"ok","data":1}'))), close=Mock())
        try:
            self.assertEqual(client.request("GET", "/test"), 1)
            self.assertEqual(client.timings, {})
        finally:
            client.close()

    def test_concurrent_read_failure_closes_clients_and_cannot_be_counted_as_success(self):
        expected = {"total": 1, "items": [{"title": "published"}]}
        for reply in ({"total": 0, "items": []}, RuntimeError("HTTP 403")):
            with self.subTest(reply=reply):
                opened = []

                def client(_port):
                    request = Mock(side_effect=reply) if isinstance(reply, Exception) else Mock(return_value=reply)
                    instance = SimpleNamespace(cookies={}, timings={}, request=request, close=Mock())
                    opened.append(instance)
                    return instance

                process = SimpleNamespace(samples=[], check=Mock())
                with patch("cms.CmsClient", side_effect=client), patch("cms.observe"):
                    with self.assertRaises(RuntimeError):
                        concurrent_reads(12345, {"front": "test-only-cookie"}, expected, process,
                                         concurrency=2, seconds=.1, interval=.01)
                self.assertEqual(len(opened), 2)
                for instance in opened:
                    instance.close.assert_called_once()

    def test_delete_inputs_use_query_instead_of_json_body(self):
        client = CmsClient(12345)
        response = SimpleNamespace(status=200, read=Mock(return_value=b'{"code":0,"message":"ok","data":true}'))
        connection = SimpleNamespace(request=Mock(), getresponse=Mock(return_value=response), close=Mock())
        client.connection = connection
        try:
            self.assertIs(client.request("DELETE", "/revoke", {"user_id": 3, "role_id": "fixture reader"}), True)
            sent = connection.request.call_args.args
            self.assertEqual(sent[:3], ("DELETE", "/revoke?user_id=3&role_id=fixture+reader", None))
        finally:
            client.close()

    def test_postgres_rbac_checks_catalog_roles_and_same_session_revocation(self):
        catalog = [
            {"component": "news", "domain": "article", "site": "admin", "action": "list",
             "method": "GET", "key": "news.article.admin.list"},
            {"component": "user", "domain": "authorization", "site": "admin", "action": "permissions",
             "method": "GET", "key": "user.authorization.admin.permissions"},
            {"component": "news", "domain": "article", "site": "front", "action": "list",
             "method": "GET", "key": "news.article.front.list"},
        ]
        identity = {"user_id": 1, "tenant_id": 2}
        member = {"user_id": 3, "tenant_id": 2, "credential_id": 4}
        login = [{"account": {"id": 3}, "tenant_id": 2, "site": site}
                 for site in ("admin", "front")]
        for failure in (False, True):
            with self.subTest(failure=failure):
                owner = SimpleNamespace(request=Mock(side_effect=[
                    [entry for entry in catalog if entry["site"] == "admin"],
                    [entry for entry in catalog if entry["site"] == "front"],
                ] + [True] * 9))
                responses = login + ([RuntimeError("missing denial")] if failure else [None] * 11)
                client = SimpleNamespace(request=Mock(side_effect=responses), close=Mock())
                with patch("cms.execute", return_value=member), patch("cms.CmsClient", return_value=client):
                    if failure:
                        with self.assertRaisesRegex(RuntimeError, "missing denial"):
                            run_postgres_rbac(Path("owned-app"), Path("owned-output"), 12345, owner, identity)
                    else:
                        result = run_postgres_rbac(Path("owned-app"), Path("owned-output"), 12345, owner, identity)
                        self.assertEqual(result["member_user_id"], 3)
                        self.assertEqual(set(result["permissions"]), {"admin_read", "admin_permissions", "front_list"})
                        self.assertNotIn("password", result)
                        calls = client.request.call_args_list[2:]
                        self.assertEqual([call.kwargs.get("expected", 200) for call in calls],
                                         [403, 403, 403, 200, 200, 200, 403, 200, 200, 403, 403])
                        saves = [call for call in owner.request.call_args_list if call.args[1].endswith("/save")]
                        self.assertEqual(owner.request.call_args_list[1].kwargs["site"], "front")
                        self.assertEqual(saves[0].args[2]["id"], saves[2].args[2]["id"])
                        self.assertNotEqual(saves[0].kwargs["site"], saves[2].kwargs["site"])
                client.close.assert_called_once()

    def test_postgres_case_keeps_source_configuration_on_build_failure(self):
        with tempfile.TemporaryDirectory(prefix="dever-cms-source-") as temporary:
            directory = Path(temporary)
            source = directory / "project"
            source.mkdir()
            for name in ("config", "module", "test"):
                (source / name).mkdir()
            original = {
                "runtime": {"mode": "all"},
                "database": {"default": {
                    "type": "postgres", "url": "postgres://fixture@127.0.0.1:12345/control",
                    "tenant_database_prefix": "dever_test_owned",
                }},
                "auth": {"providers": {"session": {"jwtSecret": "original"}}},
                "sites": {}, "log": {"level": "info"},
            }
            setting = source / "config/setting.json"
            write_private_json(setting, original)
            before = setting.read_bytes()
            staged_paths = []

            def fail_check(command, *_args, **_kwargs):
                staged = command[2]
                staged_paths.append(staged)
                self.assertNotEqual(staged, source)
                changed = json.loads((staged / "config/setting.json").read_text())
                self.assertEqual(changed["runtime"]["mode"], "api")
                self.assertNotEqual(changed["auth"]["providers"]["session"]["jwtSecret"], "original")
                raise RuntimeError("owned check failed")

            output = directory / "result"
            with patch("cms.execute", side_effect=fail_check):
                with self.assertRaisesRegex(RuntimeError, "owned check failed"):
                    run_postgres_case(source, output, directory / "compiler", articles=2)
            self.assertEqual(setting.read_bytes(), before)
            self.assertFalse((source / "cms-app").exists())
            self.assertTrue(staged_paths)
            self.assertFalse(staged_paths[0].exists())
            self.assertEqual(json.loads((output / "report.json").read_text())["status"], "failed")

    def test_private_config_replacement_is_atomic_and_keeps_exclusive_creation(self):
        with tempfile.TemporaryDirectory(prefix="dever-cms-setting-") as temporary:
            directory = Path(temporary)
            setting = directory / "setting.json"
            write_private_json(setting, {"original": True})
            with self.assertRaises(FileExistsError):
                write_private_json(setting, {"overwrite": True})
            with self.assertRaises(TypeError):
                write_private_json(setting, {"invalid": object()}, replace=True)
            self.assertEqual(json.loads(setting.read_text()), {"original": True})
            self.assertEqual(list(directory.iterdir()), [setting])
            write_private_json(setting, {"postgres": True}, replace=True)
            self.assertEqual(json.loads(setting.read_text()), {"postgres": True})
            self.assertEqual(setting.stat().st_mode & 0o777, 0o600)
            self.assertEqual(list(directory.iterdir()), [setting])

    def test_envelope_failure_cannot_count_as_publication(self):
        self.assertEqual(decode_response(b'{"code":0,"message":"ok","data":{"id":1}}', 200),
                         {"id": 1})
        for document, status in (({"code": 0, "message": "ok", "data": {}}, 500),
                                 ({"code": 409, "message": "conflict", "data": None}, 200),
                                 ({"id": 1}, 200)):
            with self.assertRaises(RuntimeError):
                decode_response(json.dumps(document).encode(), status)

    def test_isolation_preserves_site_contract_without_mutating_input(self):
        source = {
            "runtime": {"mode": "all"}, "http": {"port": 8080},
            "database": {"default": {"type": "sqlite", "path": "old.db"}},
            "auth": {"providers": {"session": {"jwtSecret": "example", "verify": "user.account.verify"}}},
            "sites": {"admin": {"path": "admin", "auth": "session", "origin": "https://example.invalid"}},
            "log": {"level": "info"},
        }
        staged = isolated_settings(source, 12345)
        self.assertEqual(source["runtime"]["mode"], "all")
        self.assertEqual(source["database"]["default"]["path"], "old.db")
        self.assertEqual(staged["runtime"]["mode"], "api")
        self.assertEqual(staged["sites"]["admin"]["path"], "admin")
        self.assertEqual(staged["sites"]["admin"]["origin"], "http://127.0.0.1:12345")
        self.assertNotEqual(staged["auth"]["providers"]["session"]["jwtSecret"], "example")
        self.assertEqual(staged["auth"]["providers"]["session"]["verify"], "user.account.verify")
        self.assertEqual(source["log"]["level"], "info")
        self.assertEqual(staged["log"]["level"], "error")
        source["database"]["default"]["type"] = "postgres"
        with self.assertRaises(RuntimeError):
            isolated_settings(source, 12345)

    def test_postgres_isolation_preserves_explicit_control_database_and_prefix(self):
        source = {
            "runtime": {"mode": "all"},
            "database": {"default": {
                "type": "postgres", "url": "postgres://fixture@127.0.0.1:12345/control",
                "tenant_database_prefix": "dever_test_owned", "tls": "disabled",
            }},
            "auth": {"providers": {"session": {"jwtSecret": "example"}}},
            "sites": {"admin": {"origin": "https://example.invalid"}},
            "log": {"level": "info"},
        }
        staged = isolated_settings(source, 23456)
        self.assertEqual(staged["database"], source["database"])
        self.assertEqual(staged["http"], {"host": "127.0.0.1", "port": 23456})
        self.assertNotIn("path", staged["database"]["default"])
        source["database"]["default"].pop("tenant_database_prefix")
        with self.assertRaises(RuntimeError):
            isolated_settings(source, 23456)

    def test_publish_workload_uses_tenant_and_title_parameters(self):
        identity = {"credential_id": 11, "user_id": 3, "tenant_id": 2}
        publication = {"slug": "bench-0", "title": "Tenant A 0 published",
                       "body": "Published body"}
        client = SimpleNamespace(request=Mock(side_effect=[
            {"account": {"id": 3}, "tenant_id": 2, "site": "admin"},
            {"account": {"id": 3}, "tenant_id": 2, "site": "front"},
            {"id": 1, "slug": "bench-0", "title": "Tenant A 0", "version": 1},
            {"id": 7, "article_id": 1, "title": "Tenant A 0 published", "version": 2},
            publication, {"total": 1, "items": [publication]}, None,
        ]))
        process = SimpleNamespace(sample=Mock())
        contract = publish_workload(client, process, identity, "fixture-password", 1,
                                    tenant_key="tenant_a", title_prefix="Tenant A")
        self.assertEqual(contract, {"articles": 1, "titles": ["Tenant A 0 published"],
                                    "duplicate_publish_status": 409})
        for call in client.request.call_args_list[:2]:
            self.assertEqual(call.args[2]["tenant"], "tenant_a")
        revision = client.request.call_args_list[3]
        self.assertEqual(revision.args[:2], ("POST", "/content/revision/admin/manage/create"))
        self.assertEqual(revision.args[2]["expected_version"], 1)
        self.assertEqual(revision.args[2]["selected_article_id"], 1)
        self.assertEqual(process.sample.call_count, 1)

    def test_small_workload_reports_samples_without_implying_p99(self):
        self.assertEqual(latency_summary({"publish": [4, 1, 3]}),
                         {"publish": {"samples": 3, "p50_ms": 3, "max_ms": 4}})

    def test_command_timeout_retains_output_and_reaps_its_owned_process(self):
        with tempfile.TemporaryDirectory(prefix="dever-cms-timeout-") as temporary:
            directory = Path(temporary)
            command = [sys.executable, "-u", "-c",
                       "import time,sys; print('starting'); print('diagnostic',file=sys.stderr); time.sleep(10)"]
            with self.assertRaises(subprocess.TimeoutExpired):
                execute(command, directory, "timeout", timeout=0.2)
            self.assertEqual((directory / "timeout.stdout.log").read_text(), "starting\n")
            self.assertEqual((directory / "timeout.stderr.log").read_text(), "diagnostic\n")


if __name__ == "__main__":
    unittest.main()

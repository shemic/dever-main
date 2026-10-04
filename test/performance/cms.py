"""显式验收双源码 CMS 的真实身份、租户、发布链及专属内存预算。"""

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from copy import deepcopy
import http.client
from http.cookies import SimpleCookie
import json
import math
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import tempfile
from threading import Event
import time
from urllib.parse import urlencode

from build import ROOT, fingerprint
from process import Budget, Process, observe, summarize, resource_trend
from settings import write_private_json

FORMATS = ("dever", "md")
ARTICLE_PATH = "/news/article/admin/manage"
ARTICLE_LIST_PATH = f"{ARTICLE_PATH}/list?size=100"
REVISION_PATH = "/content/revision/admin/manage/create"
SESSION_PATH = "/user/account/{site}/session/login"
PUBLICATIONS_PATH = "/news/article/front/browse/list?size=100"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def execute(command, directory, log_name, *, envelope=False, timeout=180):
    child = subprocess.Popen([str(value) for value in command], cwd=directory,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    failure = None
    try:
        stdout, stderr = child.communicate(timeout=timeout)
    except BaseException as error:
        failure = error
        try:
            signal_group(child.pid, signal.SIGTERM)
            stdout, stderr = child.communicate(timeout=3)
        except subprocess.TimeoutExpired:
            signal_group(child.pid, signal.SIGKILL)
            stdout, stderr = child.communicate(timeout=3)
    (directory / f"{log_name}.stdout.log").write_bytes(stdout)
    (directory / f"{log_name}.stderr.log").write_bytes(stderr)
    if failure is not None:
        raise failure
    require(child.returncode == 0, f"{log_name} exited {child.returncode}: {directory}")
    if envelope:
        return decode_response(stdout, 200)


def signal_group(pid, signal_number):
    try:
        os.killpg(pid, signal_number)
    except ProcessLookupError:
        pass


def decode_response(body, status):
    document = json.loads(body)
    require(isinstance(document, dict) and set(document) == {"code", "message", "data"},
            f"unexpected response envelope (HTTP {status})")
    require(status == 200 and document["code"] == 0,
            f"request failed: HTTP {status}, code={document['code']}, {document['message']}")
    return document["data"]


def copy_cms_project(source, destination):
    destination.mkdir()
    for name in ("config", "module", "test"):
        shutil.copytree(source / name, destination / name)


def build_cases(compiler, output):
    output.mkdir(parents=True, exist_ok=False)
    cases = {}
    for source_format in FORMATS:
        source = ROOT / "examples/cms" / source_format
        project = output / source_format
        copy_cms_project(source, project)
        started = time.monotonic()
        execute([compiler, "check", project], project, "check")
        execute([compiler, "build", project, "--output", project / "cms-app"], project, "build", timeout=900)
        executable = project / "cms-app"
        cases[source_format] = {
            "executable": str(executable), "config": str(project / "config/setting.json"),
            "fingerprint": fingerprint(executable),
            "build_seconds": time.monotonic() - started,
        }
    manifest = {"format": "dever-cms-publish-v1", "cases": cases}
    write_private_json(output / "manifest.json", manifest)
    return manifest


def available_port():
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


def isolated_settings(settings, port):
    settings = deepcopy(settings)
    settings["runtime"]["mode"] = "api"
    settings["http"] = {"host": "127.0.0.1", "port": port}
    settings["auth"]["providers"]["session"]["jwtSecret"] = secrets.token_hex(32)
    for site in settings["sites"].values():
        site["origin"] = f"http://127.0.0.1:{port}"
    database = settings["database"]["default"]
    if database["type"] == "sqlite":
        database["path"] = "data/db/platform.db"
        database["tenant_directory"] = "data/tenants"
    elif database["type"] == "postgres":
        require(bool(database.get("url")) and bool(database.get("tenant_database_prefix")),
                "PostgreSQL CMS acceptance requires an explicit isolated control database and tenant prefix")
    else:
        raise RuntimeError("CMS acceptance supports only SQLite and PostgreSQL")
    settings["log"]["level"] = "error"
    return settings


class CmsClient:
    def __init__(self, port):
        self.connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        self.origin = f"http://127.0.0.1:{port}"
        self.cookies = {}
        self.timings = {}
        self.record_timings = True

    def request(self, method, path, payload=None, *, site=None, expected=200):
        headers = {"Content-Type": "application/json", "Origin": self.origin}
        if site in self.cookies:
            headers["Cookie"] = self.cookies[site]
        body = None
        if payload is not None:
            if method in ("GET", "DELETE"):
                query = {name: value if isinstance(value, str) else json.dumps(value)
                         for name, value in payload.items()}
                path += ("&" if "?" in path else "?") + urlencode(query)
            else:
                body = json.dumps(payload)
        started = time.monotonic()
        self.connection.request(method, path, body, headers)
        response = self.connection.getresponse()
        body = response.read()
        route = path.split("?")[0]
        label = f"{method} {route}" + (f" expected {expected}" if expected != 200 else "")
        if self.record_timings:
            self.timings.setdefault(label, []).append(
                (time.monotonic() - started) * 1000)
        require(response.status == expected,
                f"{method} {path}: HTTP {response.status}, expected {expected}: {body!r}")
        if expected != 200:
            document = json.loads(body)
            require(document.get("code") != 0, "rejected request returned success")
            return None
        data = decode_response(body, response.status)
        if path.endswith("/login"):
            cookie = SimpleCookie()
            for name, value in response.getheaders():
                if name.lower() == "set-cookie":
                    cookie.load(value)
            require(bool(cookie), "login did not return a session cookie")
            self.cookies[site] = "; ".join(f"{name}={value.value}" for name, value in cookie.items())
        return data

    def close(self):
        self.connection.close()


def wait_ready(process, client):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        process.sample()
        require(process.child.poll() is None, f"CMS exited before readiness: {process.directory}")
        try:
            client.request("GET", ARTICLE_LIST_PATH, expected=401)
            return time.monotonic() - process.started
        except (ConnectionError, http.client.HTTPException, OSError):
            client.close()
            time.sleep(0.02)
    raise TimeoutError(f"CMS readiness timeout: {process.directory}")


def login_client(client, identity, password, tenant_key):
    for site in ("admin", "front"):
        data = client.request("POST", SESSION_PATH.format(site=site), {
            "credential_id": identity["credential_id"], "password": password, "tenant": tenant_key,
        }, site=site)
        require(data["account"]["id"] == identity["user_id"]
                and data["tenant_id"] == identity["tenant_id"] and data["site"] == site,
                "login changed the trusted identity or tenant/site")


def publish_workload(client, process, identity, password, articles, *,
                     tenant_key="bench", title_prefix="Article"):
    login_client(client, identity, password, tenant_key)
    titles = []
    ids = []
    for index in range(articles):
        slug, title = f"bench-{index}", f"{title_prefix} {index}"
        created = client.request("POST", f"{ARTICLE_PATH}/create", {
            "slug": f" {slug.upper()} ", "title": f" {title} ", "body": "Draft body",
        }, site="admin")
        require(created["slug"] == slug and created["title"] == title,
                "article creation did not normalize input")
        require("author_id" not in created and "status" not in created,
                "article creation leaked private owner or publication state")
        changed = client.request("POST", REVISION_PATH, {
            "selected_article_id": created["id"], "expected_version": created["version"],
            "title": f" {title} published ", "body": "Published body",
        }, site="admin")
        expected_title = f"{title} published"
        require(changed["title"] == expected_title
                and changed["article_id"] == created["id"]
                and changed["version"] == created["version"] + 1,
                "revision did not preserve normalized content and advance the article version")
        published = client.request("POST", f"{ARTICLE_PATH}/publish", {
            "article_id": created["id"],
        }, site="admin")
        require(published["slug"] == slug and published["title"] == expected_title
                and published["body"] == "Published body",
                "publication did not reflect the stored article")
        titles.append(expected_title)
        ids.append(created["id"])
        process.sample()
    listing = client.request("GET", PUBLICATIONS_PATH, site="front")
    require(listing["total"] == articles
            and sorted(value["title"] for value in listing["items"]) == sorted(titles),
            "front query did not return every published article exactly once")
    client.request("POST", f"{ARTICLE_PATH}/publish", {"article_id": ids[0]},
                   site="admin", expected=409)
    return {"articles": articles, "titles": sorted(titles), "duplicate_publish_status": 409}


def provision_identity(executable, directory, tenant_key):
    password = secrets.token_urlsafe(24)
    identity = execute([executable, "user.account.bootstrap", json.dumps({
        "tenant_key": tenant_key, "tenant_name": f"Benchmark {tenant_key}",
        "email": f"{tenant_key}@example.invalid", "display_name": "Benchmark",
        "password": password,
    })], directory, f"bootstrap-{tenant_key}", envelope=True)
    execute([executable, "--dever-tenant-migrate", identity["tenant_id"]],
            directory, f"migrate-{tenant_key}")
    for site in ("admin", "front"):
        execute([executable, "--dever-tenant-owner", identity["tenant_id"], site,
                 identity["user_id"]], directory, f"owner-{tenant_key}-{site}")
    return identity, password


def latency_summary(timings):
    result = {}
    for action, samples in timings.items():
        values = sorted(samples)
        result[action] = {"samples": len(values), "p50_ms": values[len(values) // 2],
                          "max_ms": values[-1]}
    return result


def catalog_key(catalog, component, domain, site, action, method):
    matches = [entry["key"] for entry in catalog
               if (entry["component"], entry["domain"], entry["site"],
                   entry["action"], entry["method"]) == (component, domain, site, action, method)]
    require(len(matches) == 1 and bool(matches[0]),
            f"permission catalog must uniquely identify {component}.{domain}.{site}.{action}")
    return matches[0]


def run_postgres_rbac(executable, directory, port, owner, identity):
    catalog = owner.request("GET", "/user/authorization/admin/manage/permissions", site="admin")
    front_catalog = owner.request("GET", "/user/authorization/front/manage/permissions", site="front")
    permissions = {
        "admin_read": catalog_key(catalog, "news", "article", "admin", "list", "GET"),
        "admin_permissions": catalog_key(catalog, "user", "authorization", "admin", "permissions", "GET"),
        "front_list": catalog_key(front_catalog, "news", "article", "front", "list", "GET"),
    }
    require(len(set(permissions.values())) == 3, "RBAC acceptance reused a permission key")
    password = secrets.token_urlsafe(24)
    member = execute([executable, "user.account.fixture_member", json.dumps({
        "selected_tenant_id": identity["tenant_id"], "email": f"member-{secrets.token_hex(8)}@example.invalid",
        "display_name": "Acceptance member", "password": password,
    })], directory, "rbac-member", envelope=True)
    require(isinstance(member["user_id"], int) and not isinstance(member["user_id"], bool)
            and member["user_id"] > 0 and member["user_id"] != identity["user_id"]
            and member["tenant_id"] == identity["tenant_id"],
            "RBAC acceptance did not create a distinct member in the owner's tenant")
    probes = (("admin", ARTICLE_LIST_PATH),
              ("admin", "/user/authorization/admin/manage/permissions"),
              ("front", PUBLICATIONS_PATH))
    client = CmsClient(port)
    try:
        login_client(client, member, password, "tenant_a")
        for site, path in probes:
            client.request("GET", path, site=site, expected=403)
        roles = (("admin", "fixture-reader", "admin_read"),
                 ("admin", "fixture-catalog", "admin_permissions"),
                 ("front", "fixture-reader", "front_list"))
        for site, role_id, permission in roles:
            base = f"/user/authorization/{site}/manage"
            owner.request("POST", f"{base}/save", {
                "id": role_id, "name": role_id, "all_permissions": False,
                "permission_keys": [permissions[permission]],
            }, site=site)
            owner.request("POST", f"{base}/grant", {
                "user_id": member["user_id"], "role_id": role_id,
            }, site=site)
        for site, path in probes:
            client.request("GET", path, site=site)
        owner.request("DELETE", "/user/authorization/admin/manage/revoke", {
            "user_id": member["user_id"], "role_id": "fixture-reader",
        }, site="admin")
        client.request("GET", ARTICLE_LIST_PATH, site="admin", expected=403)
        client.request("GET", probes[1][1], site="admin")
        client.request("GET", PUBLICATIONS_PATH, site="front")
        for site, role_id, path in (("admin", "fixture-catalog", probes[1][1]),
                                    ("front", "fixture-reader", PUBLICATIONS_PATH)):
            owner.request("DELETE", f"/user/authorization/{site}/manage/revoke", {
                "user_id": member["user_id"], "role_id": role_id,
            }, site=site)
            client.request("GET", path, site=site, expected=403)
    finally:
        client.close()
    return {"member_user_id": member["user_id"], "permissions": permissions,
            "before_grant_denied": True, "both_admin_roles_allowed": True,
            "revoked_role_denied": True, "second_role_still_allowed": True,
            "front_same_named_role_still_allowed": True}


class ReadLatency:
    """固定内存的对数直方图；分位数报告桶上界，最大相邻桶比例约1.011。"""

    def __init__(self):
        self.buckets = [0] * 2048
        self.count = 0

    def add(self, milliseconds):
        index = math.ceil(math.log2(1 + milliseconds) * 64)
        require(0 <= index < len(self.buckets), "CMS latency exceeds histogram bounds")
        self.buckets[index] += 1
        self.count += 1

    def merge(self, other):
        self.buckets = [left + right for left, right in zip(self.buckets, other.buckets)]
        self.count += other.count

    def percentile(self, fraction):
        threshold = math.ceil(self.count * fraction)
        count = 0
        for index, samples in enumerate(self.buckets):
            count += samples
            if count >= threshold:
                return 2 ** (index / 64) - 1
        raise ValueError("empty latency histogram")


def concurrent_reads(port, cookies, expected, process, *, concurrency, seconds, interval):
    """Each closed-loop client keeps its own connection and verifies every reply."""
    stop = Event()
    started = time.monotonic()
    deadline = started + seconds
    first = len(process.samples)

    def read_publications():
        client = CmsClient(port)
        client.cookies = cookies.copy()
        client.record_timings = False
        latency = ReadLatency()
        try:
            while not stop.is_set() and time.monotonic() < deadline:
                request_started = time.monotonic()
                listing = client.request("GET", PUBLICATIONS_PATH, site="front")
                require(listing == expected, "concurrent CMS read changed publication results")
                latency.add((time.monotonic() - request_started) * 1000)
            return latency
        except BaseException:
            stop.set()
            raise
        finally:
            client.close()

    with ThreadPoolExecutor(max_workers=concurrency) as clients:
        futures = [clients.submit(read_publications) for _ in range(concurrency)]
        try:
            while any(not future.done() for future in futures):
                process.check()
                observe([process], interval, interval)
            elapsed = time.monotonic() - started
            latency = ReadLatency()
            for future in futures:
                latency.merge(future.result())
        finally:
            stop.set()
    require(latency.count > 0, "concurrent CMS read completed without a successful request")
    require(elapsed >= seconds, "concurrent CMS read ended before the measurement window")
    process.sample()
    return {"mode": "closed_loop_authenticated_read", "concurrency": concurrency,
            "requested_seconds": seconds, "elapsed_seconds": elapsed,
            "succeeded": latency.count, "errors": 0, "result_checked_on_every_request": True,
            "actual_success_qps": latency.count / elapsed,
            "latency_measurement": "request and reply validation; logarithmic bucket upper bound, 64 buckets per octave",
            "request_latency_ms": {name: latency.percentile(fraction)
                                   for name, fraction in (("p50", .5), ("p95", .95), ("p99", .99))},
            "resources": summarize(process.samples[first:]),
            "trend": resource_trend(process.samples[first:]) if len(process.samples[first:]) >= 4 else None}


def sqlite_pool_descriptor_limits(settings, directory):
    database = settings["database"]["default"]
    require(database["type"] == "sqlite", "CMS read benchmark requires isolated SQLite")
    databases = [directory / database["path"]]
    databases.extend((directory / database["tenant_directory"]).glob("*.db"))
    # 每个连接可持有主文件与WAL；同进程各连接共用一个SHM句柄。
    return {str(path.resolve()) + suffix: limit for path in databases
            for suffix, limit in (("", database["max_connections"]),
                                  ("-wal", database["max_connections"]), ("-shm", 1))}


def validate_read_descriptor_recovery(load):
    before = Counter(load["descriptors_before"].values())
    after = Counter(load["descriptors_after"].values())
    for path, limit in load["pooled_descriptor_limits"].items():
        require(after[path] <= limit, f"CMS SQLite descriptor count exceeds pool limit: {path}")
        before.pop(path, None)
        after.pop(path, None)
    require(before == after,
            f"CMS read descriptors did not recover: added={dict(after - before)}, missing={dict(before - after)}")


def run_case(case, directory, *, articles, cgroup_parent, memory_mib,
             idle_seconds=0.2, interval=0.05, server_cpu=None, cpu_quota=None,
             concurrency=0, read_seconds=5):
    executable = Path(case["executable"])
    require(fingerprint(executable) == case["fingerprint"], "CMS artifact fingerprint changed")
    directory.mkdir(parents=True, exist_ok=False)
    target = directory / "cms-app"
    shutil.copy2(executable, target)
    (directory / "config").mkdir()
    (directory / "data").mkdir()
    port = available_port()
    settings = isolated_settings(json.loads(Path(case["config"]).read_text()), port)
    write_private_json(directory / "config/setting.json", settings)
    identity, password = provision_identity(target, directory, "bench")
    budget = Budget(cgroup_parent, memory_mib, cpu_quota)
    client = CmsClient(port)
    failure = None
    result = {}
    try:
        with Process([target], directory / "server", budget=budget,
                     cpus=(server_cpu,) if server_cpu is not None else None) as process:
            result["raw"] = process.samples
            ready_seconds = wait_ready(process, client)
            started = time.monotonic()
            contract = publish_workload(client, process, identity, password, articles)
            elapsed = time.monotonic() - started
            result.update(contract=contract, ready_seconds=ready_seconds,
                          workload_seconds=elapsed, published_articles_per_second=articles / elapsed,
                          latency=latency_summary(client.timings))
            if concurrency:
                expected = client.request("GET", PUBLICATIONS_PATH, site="front")
                # 控制连接不参与长读负载，先关闭，避免其空闲超时改变基线。
                client.close()
                observe([process], 0.3, interval)
                descriptors_before = process.descriptors()
                pooled_limits = sqlite_pool_descriptor_limits(settings, directory)
                read_load = {"descriptors_before": descriptors_before,
                             "pooled_descriptor_limits": pooled_limits}
                result["concurrent_reads"] = read_load
                read_load.update(concurrent_reads(port, client.cookies, expected, process,
                    concurrency=concurrency, seconds=read_seconds, interval=interval))
                observe([process], 0.3, interval)
                descriptors_after = process.descriptors()
                read_load.update(descriptors_after=descriptors_after,
                                 fd_delta_after_disconnect=len(descriptors_after) - len(descriptors_before))
                validate_read_descriptor_recovery(read_load)
            observe([process], idle_seconds, interval)
            process.sample()
            process.check()
            require(process.child.poll() is None, "CMS exited during the publish workload")
            resources = summarize(process.samples)
            result.update(resources=resources, budget_before_stop=budget.sample())
    except BaseException as error:
        failure = error
    finally:
        client.close()
        try:
            budget.close()
        except BaseException as error:
            if failure is None:
                failure = error
            else:
                failure.add_note(f"cannot close owned CMS budget: {error}")
    result["cgroup"] = budget.final
    if failure is None and budget.final is not None and (
            budget.final["events"]["oom"] or budget.final["events"]["oom_kill"]):
        failure = RuntimeError("CMS exceeded its memory budget")
    result["status"] = "failed" if failure else "passed"
    if failure is not None:
        result["error"] = str(failure)
    write_private_json(directory / "report.json", result)
    if failure is not None:
        raise failure
    return result


def run_postgres_case(project, output, compiler, *, articles, rbac_fixture=False):
    # 构建、临时密钥和端口只写测试副本，调用者的项目配置始终保持不变。
    with tempfile.TemporaryDirectory(prefix="dever-cms-postgres-") as temporary:
        staged = Path(temporary) / "project"
        copy_cms_project(project, staged)
        return run_staged_postgres_case(staged, output, compiler, articles=articles,
                                        rbac_fixture=rbac_fixture)


def run_staged_postgres_case(project, output, compiler, *, articles, rbac_fixture):
    require(1 <= articles <= 100, "PostgreSQL CMS acceptance requires 1..100 articles")
    output.mkdir(parents=True, exist_ok=False)
    report = {"status": "running", "source": "dever", "tenants": {},
              "scope": "PostgreSQL HTTP owner flow and physical database tenant isolation; not non-owner RBAC revocation"}
    if rbac_fixture:
        report["scope"] = "PostgreSQL HTTP owner flow, physical database tenant isolation and non-owner multi-role/site grant-revoke"
    clients = {}
    try:
        setting_path = project / "config/setting.json"
        initial = json.loads(setting_path.read_text())
        require(initial["database"]["default"]["type"] == "postgres",
                "postgres-case requires a PostgreSQL project configuration")
        port = available_port()
        write_private_json(setting_path, isolated_settings(initial, port), replace=True)
        execute([compiler, "check", project], output, "check")
        executable = project / "cms-app"
        execute([compiler, "build", project, "--output", executable], output, "build", timeout=900)
        report["binary"] = fingerprint(executable)
        identities = {key: provision_identity(executable, output, key)
                      for key in ("tenant_a", "tenant_b")}
        require(identities["tenant_a"][0]["tenant_id"] != identities["tenant_b"][0]["tenant_id"],
                "PostgreSQL acceptance reused a tenant identity")
        clients = {key: CmsClient(port) for key in identities}
        with Process([executable], output / "server") as process:
            report["ready_seconds"] = wait_ready(process, clients["tenant_a"])
            for key, (identity, password) in identities.items():
                contract = publish_workload(clients[key], process, identity, password, articles,
                                            tenant_key=key, title_prefix=key)
                permissions = clients[key].request(
                    "GET", "/user/authorization/admin/manage/permissions", site="admin")
                require(bool(permissions), "CMS permission catalog is empty")
                report["tenants"][key] = {"tenant_id": identity["tenant_id"],
                                          "user_id": identity["user_id"], "contract": contract}
            for key, client in clients.items():
                listing = client.request("GET", PUBLICATIONS_PATH, site="front")
                contract = report["tenants"][key]["contract"]
                require(listing["total"] == articles
                        and sorted(value["title"] for value in listing["items"]) == contract["titles"],
                        "PostgreSQL tenant query leaked another tenant's articles")
                require(sorted(value["slug"] for value in listing["items"])
                        == sorted(f"bench-{index}" for index in range(articles)),
                        "PostgreSQL tenants did not preserve the same slug namespace")
                report["tenants"][key]["slugs"] = sorted(value["slug"] for value in listing["items"])
            identity, password = identities["tenant_a"]
            clients["tenant_a"].request("POST", SESSION_PATH.format(site="admin"), {
                "credential_id": identity["credential_id"], "password": password, "tenant": "tenant_b",
            }, site="admin", expected=401)
            front_cookie = clients["tenant_a"].cookies["front"]
            clients["tenant_a"].cookies["front"] = clients["tenant_a"].cookies["admin"]
            clients["tenant_a"].request("GET", PUBLICATIONS_PATH, site="front", expected=401)
            clients["tenant_a"].cookies["front"] = front_cookie
            if rbac_fixture:
                report["rbac"] = run_postgres_rbac(executable, output, port,
                                                   clients["tenant_a"], identity)
            process.sample()
            process.check()
            require(process.child.poll() is None, "PostgreSQL CMS exited during tenant acceptance")
            report["resources"] = summarize(process.samples)
        report["cross_site_cookie_rejected"] = True
        report["cross_tenant_login_rejected"] = True
        report["status"] = "passed"
    except BaseException as error:
        report["status"] = "failed"
        report["error"] = str(error)
        raise
    finally:
        for client in clients.values():
            client.close()
        write_private_json(output / "report.json", report)
    return report


def run_cases(manifest, output, *, articles, cgroup_parent, memory_mib,
              concurrency=0, read_seconds=5):
    require(manifest["format"] == "dever-cms-publish-v1", "unexpected CMS manifest format")
    require(1 <= articles <= 100, "articles must be 1..100")
    require(0 <= concurrency <= 64, "read concurrency must be 0..64")
    require(math.isfinite(read_seconds) and 0.1 <= read_seconds <= 3600,
            "read seconds must be finite and within 0.1..3600")
    require((memory_mib is None) == (cgroup_parent is None),
            "memory budget and cgroup parent must be provided together")
    output.mkdir(parents=True, exist_ok=False)
    report = {"format": manifest["format"], "memory_mib": memory_mib,
              "scope": "owned loopback HTTP/1, sequential publish, isolated SQLite; not a saturation result",
              "cases": {}}
    if concurrency:
        report["scope"] = "owned loopback HTTP/1, sequential publish and closed-loop authenticated reads, isolated SQLite"
    try:
        for source_format in FORMATS:
            report["cases"][source_format] = run_case(manifest["cases"][source_format],
                output / source_format, articles=articles,
                cgroup_parent=cgroup_parent, memory_mib=memory_mib,
                concurrency=concurrency, read_seconds=read_seconds)
        require(report["cases"]["dever"]["contract"] == report["cases"]["md"]["contract"],
                "Dever and Markdown CMS changed the publish contract")
        report["status"] = "passed"
    except BaseException as error:
        report["status"] = "failed"
        report["error"] = str(error)
        raise
    finally:
        for source_format in FORMATS:
            case_report = output / source_format / "report.json"
            if source_format not in report["cases"] and case_report.is_file():
                report["cases"][source_format] = json.loads(case_report.read_text())
        write_private_json(output / "report.json", report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    build = actions.add_parser("build")
    build.add_argument("--compiler", type=Path, default=ROOT / "target/debug/dever")
    build.add_argument("--output", type=Path, required=True)
    run = actions.add_parser("run")
    run.add_argument("--manifest", type=Path, required=True)
    run.add_argument("--output", type=Path, required=True)
    run.add_argument("--articles", type=int, default=16)
    run.add_argument("--cgroup-parent", type=Path)
    run.add_argument("--memory-mib", type=int, choices=(64, 128))
    run.add_argument("--concurrency", type=int, default=0,
                     help="0 disables load; otherwise 1..64 independent authenticated read clients")
    run.add_argument("--read-seconds", type=float, default=5)
    postgres = actions.add_parser("postgres-case")
    postgres.add_argument("--project", type=Path, required=True)
    postgres.add_argument("--output", type=Path, required=True)
    postgres.add_argument("--compiler", type=Path, default=ROOT / "target/debug/dever")
    postgres.add_argument("--articles", type=int, default=2)
    postgres.add_argument("--rbac-fixture", action="store_true",
                          help="run test-only member CMD provided by the Rust PostgreSQL fixture")
    args = parser.parse_args()
    if args.action == "build":
        result = build_cases(args.compiler.resolve(strict=True), args.output.resolve())
    elif args.action == "postgres-case":
        report = run_postgres_case(args.project.resolve(strict=True), args.output.resolve(),
                                   args.compiler.resolve(strict=True), articles=args.articles,
                                   rbac_fixture=args.rbac_fixture)
        result = {"status": report["status"], "report": str(args.output.resolve() / "report.json")}
    else:
        result = run_cases(json.loads(args.manifest.read_text()), args.output.resolve(),
                           articles=args.articles, cgroup_parent=args.cgroup_parent,
                           memory_mib=args.memory_mib, concurrency=args.concurrency,
                           read_seconds=args.read_seconds)
        result = {"status": result["status"], "memory_mib": result["memory_mib"],
                  "report": str(args.output / "report.json")}
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()

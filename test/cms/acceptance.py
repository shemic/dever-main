"""自有 SQLite/HTTP CMS 业务验收；不启动压测，不使用部署数据库。"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing, contextmanager
from datetime import datetime, timezone
import http.client
import json
from pathlib import Path
import secrets
import shutil
import sqlite3
import sys
import tempfile
from threading import Barrier
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "test/performance"))
from cms import (CmsClient, available_port, catalog_key, copy_cms_project,
                 decode_response, execute, isolated_settings, login_client,
                 provision_identity, require)
from process import Process
from settings import write_private_json

ARTICLE = "/news/article/admin/manage"
REVISION = "/content/revision/admin/manage/create"
CATEGORY = "/content/category/admin/manage"
ASSET = "/media/asset/admin/manage/upload"
SCHEDULE = "/publishing/schedule/admin/manage/schedule"
ROLES = "/user/authorization/admin/manage"
BUSINESS_TABLES = ("article", "revision", "publication", "schedule", "operation", "asset", "category")


def append_member_fixture(project):
    """复用真实凭据/成员关系 fixture；仅追加到本运行器复制的 plain 项目。"""
    fixtures = ROOT / "test/dever-tests/fixtures/postgres_api"
    for target, name in (("user/account/app.dever", "account_app.dever"),
                         ("user/membership/app.dever", "membership_app.dever"),
                         ("user/account/api.dever", "account_api.dever")):
        path = project / "module" / target
        require(path.is_file(), "compiler mode requires the plain CMS project")
        with path.open("a") as destination:
            destination.write("\n" + (fixtures / name).read_text())


def sql_read(database, sql, parameters=()):
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True, timeout=5)) as connection:
        connection.row_factory = sqlite3.Row
        return [dict(row) for row in connection.execute(sql, parameters)]


def snapshot(database):
    return {table: sql_read(database, f'SELECT * FROM "{table}" ORDER BY id')
            for table in BUSINESS_TABLES}


def fixture_write(database, sql, parameters=()):
    # 此入口只接收自有临时数据库。所有正常业务创建和授权仍走真实 API/CMD。
    with closing(sqlite3.connect(database, timeout=5)) as connection:
        with connection:
            connection.execute(sql, parameters)


@contextmanager
def reject_inserts(database, table):
    require(table in BUSINESS_TABLES, "unsupported failure injection table")
    fixture_write(database, f'''CREATE TRIGGER cms_reject_insert BEFORE INSERT ON "{table}"
        BEGIN SELECT RAISE(ABORT, 'owned CMS acceptance failure'); END''')
    try:
        yield
    finally:
        fixture_write(database, "DROP TRIGGER cms_reject_insert")


def wait_for(check, process, label, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        require(process.child.poll() is None, f"{label}: child exited; see {process.directory}")
        value = check()
        if value:
            return value
        time.sleep(0.05)
    raise TimeoutError(f"{label}: timed out after {timeout}s")


def ready(process, client):
    def probe():
        try:
            client.request("GET", ARTICLE + "/list", expected=401)
            return True
        except (OSError, http.client.HTTPException):
            client.close()
            return False
    wait_for(probe, process, "API startup", timeout=20)


def multipart(client, content, *, expected=200, field="file"):
    boundary = "cms-" + secrets.token_hex(12)
    body = (f'--{boundary}\r\nContent-Disposition: form-data; name="{field}"; '
            'filename="article.txt"\r\nContent-Type: text/plain\r\n\r\n').encode()
    body += content + f"\r\n--{boundary}--\r\n".encode()
    client.connection.request("POST", ASSET, body, {
        "Content-Type": f"multipart/form-data; boundary={boundary}",
        "Origin": client.origin, "Cookie": client.cookies.get("admin", ""),
    })
    response = client.connection.getresponse()
    reply = response.read()
    require(response.status == expected,
            f"upload: HTTP {response.status}, expected {expected}: {reply!r}")
    if expected != 200:
        require(json.loads(reply)["code"] != 0, "upload rejection returned success")
        return None
    return decode_response(reply, response.status)


class Acceptance:
    def __init__(self, executable, directory, settings, identity, password, member, member_password):
        self.executable = executable
        self.directory = directory
        self.settings = settings
        self.identity = identity
        self.password = password
        self.member_identity = member
        self.member_password = member_password
        self.port = settings["http"]["port"]
        self.owner = CmsClient(self.port)
        self.member = CmsClient(self.port)
        self.anonymous = CmsClient(self.port)
        self.database = directory / "data/tenants" / f"tenant_{identity['tenant_id']}.db"
        self.worker_number = 0
        self.checks = []

    def close(self):
        for client in (self.owner, self.member, self.anonymous):
            client.close()

    def passed(self, name):
        self.checks.append(name)
        print(f"PASS {name}", flush=True)

    def assert_unchanged(self, before, label):
        require(snapshot(self.database) == before, f"{label}: business rows changed")

    def create(self, client, slug, **fields):
        return client.request("POST", ARTICLE + "/create", {
            "slug": slug, "title": f"Title {slug}", "body": "Draft body", **fields,
        }, site="admin")

    def edit(self, client, article, *, title="Edited title", expected=200):
        revision = client.request("POST", REVISION, {
            "selected_article_id": article["id"], "title": title, "body": "Edited body",
            "expected_version": article["version"], "category_id": article.get("category_id"),
            "asset_id": article.get("asset_id"),
        }, site="admin", expected=expected)
        if expected != 200:
            return None
        current = client.request("GET", ARTICLE + "/detail", {"id": article["id"]}, site="admin")
        require(revision["article_id"] == current["id"] and revision["version"] == current["version"]
                and revision["title"] == current["title"] and revision["body"] == current["body"],
                "revision and current article disagree")
        return current

    def schedule(self, client, article, milliseconds):
        instant = datetime.fromtimestamp(milliseconds / 1000, timezone.utc)
        return client.request("POST", SCHEDULE, {
            "article_id": article["id"],
            "publish_at": instant.isoformat(timespec="milliseconds").replace("+00:00", "Z"),
        }, site="admin")

    def job(self, schedule):
        rows = sql_read(self.database,
                        "SELECT * FROM _dever_jobs WHERE target=? ORDER BY id",
                        ("publishing.schedule.job.publish",))
        matches = [row for row in rows if json.loads(row["payload"])["schedule_id"] == schedule["id"]]
        require(len(matches) == 1, f"schedule {schedule['id']} must own exactly one Job: {rows}")
        return matches[0]

    @contextmanager
    def worker(self):
        self.worker_number += 1
        settings = {**self.settings, "runtime": {**self.settings["runtime"], "mode": "worker"}}
        write_private_json(self.directory / "config/setting.json", settings, replace=True)
        try:
            with Process([self.executable], self.directory / f"worker-{self.worker_number}") as process:
                yield process
            require(process.child.returncode == 0, f"Worker did not shut down cleanly: {process.directory}")
        finally:
            write_private_json(self.directory / "config/setting.json", self.settings, replace=True)

    def authorization(self):
        login_client(self.owner, self.identity, self.password, "cms")
        login_client(self.member, self.member_identity, self.member_password, "cms")
        before = snapshot(self.database)
        self.anonymous.request("GET", ARTICLE + "/list", expected=401)
        self.member.request("GET", ARTICLE + "/list", site="admin", expected=403)
        for client, status in ((self.anonymous, 401), (self.member, 403)):
            client.request("POST", ARTICLE + "/create", {
                "slug": "must-not-create", "title": "Rejected", "body": "Rejected",
            }, site="admin", expected=status)
        self.assert_unchanged(before, "unauthenticated and ungranted writes")
        catalog = self.owner.request("GET", ROLES + "/permissions", site="admin")
        selections = (("news", "article", "create", "POST"),
                      ("news", "article", "list", "GET"),
                      ("news", "article", "detail", "GET"),
                      ("news", "article", "publish", "POST"),
                      ("news", "article", "remove", "DELETE"),
                      ("content", "revision", "create", "POST"),
                      ("publishing", "schedule", "schedule", "POST"))
        keys = [catalog_key(catalog, component, domain, "admin", action, method)
                for component, domain, action, method in selections]
        self.owner.request("POST", ROLES + "/save", {
            "id": "cms-editor", "name": "CMS editor", "all_permissions": False,
            "permission_keys": keys,
        }, site="admin")
        self.owner.request("POST", ROLES + "/grant", {
            "user_id": self.member_identity["user_id"], "role_id": "cms-editor",
        }, site="admin")
        self.member.request("GET", ARTICLE + "/list", {"size": 20}, site="admin")
        self.passed("real login, ungranted denial and explicit member role")

    def editorial(self):
        category = self.owner.request("POST", CATEGORY + "/create", {"name": " CMS News "}, site="admin")
        upload = multipart(self.owner, b"CMS attachment\n")
        stored = self.directory / "data/upload" / upload["storage_key"]
        require(stored.read_bytes() == b"CMS attachment\n", "uploaded bytes changed")
        files = sorted(path.name for path in stored.parent.iterdir())
        rows = snapshot(self.database)
        multipart(self.owner, b"x" * 2049, expected=400)
        multipart(self.owner, b"bad field", field="unknown", expected=400)
        require(sorted(path.name for path in stored.parent.iterdir()) == files,
                "rejected upload persisted a file")
        require(not list((self.directory / "data/tmp").iterdir()), "upload temporary files leaked")
        self.assert_unchanged(rows, "rejected uploads")
        article = self.create(self.owner, "linked-article", category_id=category["id"], asset_id=upload["id"])
        require(article["category_id"] == category["id"] and article["asset_id"] == upload["id"],
                "article lost category/asset references")
        before = snapshot(self.database)
        self.edit(self.member, article, expected=403)
        self.member.request("POST", ARTICLE + "/publish", {"article_id": article["id"]},
                            site="admin", expected=403)
        self.member.request("DELETE", ARTICLE + "/remove", {"id": article["id"]},
                            site="admin", expected=403)
        self.member.request("POST", SCHEDULE, {
            "article_id": article["id"],
            "publish_at": datetime.fromtimestamp(time.time() + 30, timezone.utc).isoformat(timespec="milliseconds"),
        }, site="admin", expected=403)
        self.assert_unchanged(before, "non-author mutation")
        edited = self.edit(self.owner, article)
        require(edited["version"] == article["version"] + 1, "edit did not advance version")
        before = snapshot(self.database)
        self.edit(self.owner, article, expected=409)
        self.assert_unchanged(before, "stale edit")
        self.passed("multipart cleanup, attachment references, author and stale-version rejection")

        # 失败发生在文章写入之后，必须连同版本/修订一起回滚。
        with reject_inserts(self.database, "revision"):
            self.edit(self.owner, edited, title="Must roll back", expected=500)
        self.assert_unchanged(before, "revision insert failure")
        self.passed("HTTP edit failure rolls back article and revision")
        winner = self.concurrent_edit(edited)
        before = snapshot(self.database)
        with reject_inserts(self.database, "publication"):
            self.owner.request("POST", ARTICLE + "/publish", {"article_id": article["id"]},
                               site="admin", expected=500)
        self.assert_unchanged(before, "publication insert failure")
        published = self.owner.request("POST", ARTICLE + "/publish", {"article_id": article["id"]}, site="admin")
        require(published["title"] == winner["title"] and published["body"] == winner["body"],
                "publication is not the winning stored revision")
        before = snapshot(self.database)
        self.owner.request("POST", ARTICLE + "/publish", {"article_id": article["id"]}, site="admin", expected=409)
        self.assert_unchanged(before, "repeated manual publish")
        self.owner.request("DELETE", CATEGORY + "/disable", {"id": category["id"]}, site="admin")
        before = snapshot(self.database)
        self.owner.request("POST", ARTICLE + "/create", {
            "slug": "disabled-category", "title": "Rejected", "body": "Rejected", "category_id": category["id"],
        }, site="admin", expected=409)
        self.assert_unchanged(before, "disabled category")
        self.passed("publish snapshot, late-failure rollback, repeat conflict and disabled category")

    def concurrent_edit(self, article):
        barrier = Barrier(2, timeout=5)
        def compete(index):
            client = CmsClient(self.port)
            client.cookies = self.owner.cookies.copy()
            try:
                barrier.wait()
                payload = {"selected_article_id": article["id"], "title": f"Concurrent {index}",
                           "body": "Winning body", "expected_version": article["version"],
                           "category_id": article["category_id"], "asset_id": article["asset_id"]}
                client.connection.request("POST", REVISION, json.dumps(payload), {
                    "Content-Type": "application/json", "Origin": client.origin,
                    "Cookie": client.cookies["admin"],
                })
                response = client.connection.getresponse()
                body = response.read()
                document = json.loads(body)
                require(set(document) == {"code", "message", "data"}, "invalid race envelope")
                require((response.status, document["code"]) in ((200, 0), (409, 409)),
                        f"unexpected concurrent edit: {response.status} {body!r}")
                return response.status, document["data"]
            finally:
                client.close()
        prior = len(sql_read(self.database, "SELECT id FROM revision WHERE article_id=?", (article["id"],)))
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(compete, (1, 2)))
        require(sorted(status for status, _ in results) == [200, 409], "concurrent edit must have one winner")
        winner = next(value for status, value in results if status == 200)
        require(winner["version"] == article["version"] + 1, "concurrent version advanced twice")
        revisions = sql_read(self.database, "SELECT * FROM revision WHERE article_id=? ORDER BY version", (article["id"],))
        require(len(revisions) == prior + 1 and revisions[-1]["title"] == winner["title"],
                "concurrent edit persisted duplicate or mismatched revisions")
        self.passed("two simultaneous edits produce one version and one revision")
        return winner

    def scheduled_publish(self):
        articles = [self.create(self.owner, f"scheduled-{index}") for index in (1, 2)]
        due = int(time.time() * 1000) + 3000
        schedules = [self.schedule(self.owner, article, due) for article in articles]
        jobs = [self.job(schedule) for schedule in schedules]
        require(len({job["id"] for job in jobs}) == 2 and all(job["run_at"] == due for job in jobs),
                "same-time schedules merged or enqueue_at lost the due time")
        before = snapshot(self.database)
        with self.worker() as worker:
            require(time.time() * 1000 < due - 500, "fixture did not leave a pre-due observation window")
            while time.time() * 1000 < due - 300:
                require(worker.child.poll() is None, "worker stopped before due time")
                require(all(self.job(value)["state"] == "pending" for value in schedules), "Job ran early")
                self.assert_unchanged(before, "not-yet-due schedule")
                time.sleep(0.05)
            wait_for(lambda: all(self.job(value)["state"] == "succeeded" for value in schedules), worker,
                     "same-time scheduled publications")
        for article in articles:
            require(len(sql_read(self.database, "SELECT id FROM publication WHERE slug=?", (article["slug"],))) == 1,
                    "scheduled article was not published exactly once")
        self.passed("real future enqueue_at and independent same-time publications")
        before = snapshot(self.database)
        for schedule in schedules:
            # 重放真实已完成任务；保留原身份/租户/权限/指纹/payload，不伪造执行身份。
            fixture_write(self.database, "UPDATE _dever_jobs SET state='pending',attempt=0,run_at=?,"
                          "claim_token='',lease_until=0,error='' WHERE id=? AND state='succeeded'",
                          (int(time.time() * 1000), self.job(schedule)["id"]))
        with self.worker() as worker:
            wait_for(lambda: all(self.job(value)["state"] == "succeeded" for value in schedules), worker,
                     "completed task replay")
        self.assert_unchanged(before, "completed task replay")
        self.passed("completed Job replay adds no publication or audit")

    def retry_and_revocation(self):
        article = self.create(self.owner, "retry-publish")
        schedule = self.schedule(self.owner, article, int(time.time() * 1000) + 500)
        before = snapshot(self.database)
        with reject_inserts(self.database, "publication"):
            with self.worker() as worker:
                wait_for(lambda: self.job(schedule)["attempt"] == 1
                         and self.job(schedule)["state"] == "pending", worker, "first failed Job attempt")
        failed = self.job(schedule)
        require(failed["error"] == "handler_failed", "fixture did not reach business failure")
        self.assert_unchanged(before, "failed Job transaction")
        with self.worker() as worker:
            wait_for(lambda: self.job(schedule)["state"] == "succeeded", worker, "automatic delayed retry")
        require(self.job(schedule)["attempt"] == 2, "retry did not use exactly one new claim")
        self.passed("Job late failure rolls back publication/audit/status and retries successfully")

        article = self.create(self.member, "revoked-publish")
        schedule = self.schedule(self.member, article, int(time.time() * 1000) + 500)
        before = snapshot(self.database)
        self.owner.request("DELETE", ROLES + "/revoke", {
            "user_id": self.member_identity["user_id"], "role_id": "cms-editor",
        }, site="admin")
        self.member.request("GET", ARTICLE + "/list", site="admin", expected=403)
        with self.worker() as worker:
            wait_for(lambda: self.job(schedule)["state"] == "blocked", worker, "revoked queued permission")
        require(self.job(schedule)["error"] == "identity_rejected", "Job blocked for the wrong reason")
        self.assert_unchanged(before, "permission revoked before Job execution")
        self.passed("immediate role revocation blocks queued Job before business writes")

    def session_lifecycle(self):
        before = snapshot(self.database)
        logged_out = self.owner.request("POST", "/user/account/admin/session/logout", {}, site="admin")
        require(logged_out["revoked"], "logout did not revoke the session")
        # 保留旧 Cookie 再请求，证明服务端撤销而非仅清除浏览器 Cookie。
        self.owner.request("GET", ARTICLE + "/list", site="admin", expected=401)
        login_client(self.owner, self.identity, self.password, "cms")
        self.owner.request("GET", ARTICLE + "/list", {"size": 20}, site="admin")
        control = self.directory / "data/db/platform.db"
        expired = int(time.time() * 1000) - 1
        fixture_write(control, "UPDATE session SET expires_at=? WHERE user_id=? AND site='admin'",
                      (expired, self.identity["user_id"]))
        self.owner.request("GET", ARTICLE + "/list", site="admin", expected=401)
        self.owner.request("GET", "/news/article/front/browse/list", {"size": 100}, site="front")
        self.assert_unchanged(before, "revoked and expired sessions")
        self.passed("logout invalidates the old cookie; expired admin session preserves front isolation")

    def run(self):
        with Process([self.executable], self.directory / "api") as process:
            ready(process, self.anonymous)
            self.authorization()
            self.editorial()
            self.scheduled_publish()
            self.retry_and_revocation()
            self.session_lifecycle()
        require(process.child.returncode == 0, f"API did not shut down cleanly: {process.directory}")


def run(args, directory, report):
    project = directory / "project"
    copy_cms_project(args.project, project)
    # 忽略输入工程的任何数据库地址；本验收只生成自有 SQLite 配置。
    settings = json.loads((project / "config/setting.json").read_text())
    settings["database"] = {"default": {"type": "sqlite", "path": "data/db/platform.db",
                                        "tenant_directory": "data/tenants", "max_connections": 4,
                                        "max_page_size": 100}}
    settings = isolated_settings(settings, available_port())
    settings["runtime"] = {"mode": "api", "shutdown_ms": 3000}
    settings["job"] = {"workers": 1, "poll_ms": 50, "lease_ms": 60000,
                       "retry_base_ms": 2000, "retry_max_ms": 2000}
    settings["http"]["upload"] = {"file_bytes": 2048}
    write_private_json(project / "config/setting.json", settings, replace=True)
    if args.compiler:
        append_member_fixture(project)
        execute([args.compiler, "check", project], project, "check")
        execute([args.compiler, "build", project, "--output", project / "cms-app"], project, "build", timeout=300)
    else:
        shutil.copy2(args.executable, project / "cms-app")
    executable = project / "cms-app"
    # 二进制不含部署配置或fixture凭据，可复用以免调试断言时反复编译。
    shutil.copy2(executable, args.output / "cms-app")
    report["executable_bytes"] = executable.stat().st_size
    identity, password = provision_identity(executable, project, "cms")
    member_password = secrets.token_urlsafe(24)
    member = execute([executable, "user.account.fixture_member", json.dumps({
        "selected_tenant_id": identity["tenant_id"], "email": "member@example.invalid",
        "display_name": "CMS member", "password": member_password,
    })], project, "member", envelope=True)
    acceptance = Acceptance(executable, project, settings, identity, password, member, member_password)
    report["passed"] = acceptance.checks
    try:
        acceptance.run()
        return {"passed": acceptance.checks, "database": "owned SQLite", "load_test": False}
    finally:
        acceptance.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--compiler", type=Path, help="explicit Dever compiler; builds an owned plain source copy")
    source.add_argument("--executable", type=Path, help="prepared CMS executable including fixture_member CMD")
    parser.add_argument("--project", type=Path, default=ROOT / "examples/cms/dever")
    parser.add_argument("--output", type=Path, required=True, help="new directory for credential-free report/logs")
    args = parser.parse_args()
    args.project = args.project.resolve(strict=True)
    for name in ("compiler", "executable"):
        if getattr(args, name):
            setattr(args, name, getattr(args, name).resolve(strict=True))
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"success": False}
    with tempfile.TemporaryDirectory(prefix="dever-cms-acceptance-") as temporary:
        directory = Path(temporary)
        try:
            report.update(run(args, directory, report), success=True)
        except BaseException as error:
            report["error"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            for log in directory.rglob("*.log"):
                destination = args.output / log.relative_to(directory)
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(log, destination)
            write_private_json(args.output / "report.json", report)
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()

"""构建可搬运的基准产物；构建耗时与运行测量分开。"""

from copy import deepcopy
import hashlib
import json
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time
from urllib.parse import urlsplit

from settings import PEER_CONFIGURATION, stage_peer, write_private_json

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = Path(__file__).parent / "fixtures"
TLS = ROOT / "test/dever-tests/fixtures/tls"
SETTING_PATH = ROOT / "config/setting.json"
POSTGRES_TEST_CONNECTION = "postgres_test"
APPLICATION_CONFIGURATION = "llvm-cmd-v1"
ENTRIES = {
    "runtime": ("sync_idle", "async_idle", "tasks", "tasks_on_worker", "channels", "cancel"),
    "http": ("http", "https", "http2", "https2"),
    "live": ("tcp", "ws", "sse"),
}
ORM_ENTRIES = ("orm_idle", "orm_crud", "orm_list", "orm_cursor", "orm_stream",
               "orm_pool", "orm_http")
RUNTIME_PROFILES = {
    "base": ("runtime-api", "runtime-external"),
    "sqlite": ("runtime-api", "runtime-external", "runtime-sqlite"),
    "postgres": ("runtime-api", "runtime-external", "runtime-postgres"),
    "both": ("runtime-api", "runtime-external", "runtime-sqlite", "runtime-postgres"),
}
BENCHMARK_COMMAND = "benchmark.command.run"
CONFIG_FUNCTIONS = {
    "runtime": ("lifetime", "idle_sample_ms", "pending", "channel_capacity", "input", "pending_input"),
    "http": ("lifetime", "limits", "http2_limits", "large_body", "certificate", "key"),
    "live": (),
    "orm": ("lifetime", "orm_input", "orm_rows", "orm_concurrency", "orm_cancel_work"),
}


def fingerprint(path):
    with path.open("rb") as binary:
        digest = hashlib.file_digest(binary, "sha256").hexdigest()
    return {"sha256": digest, "bytes": path.stat().st_size}


def postgres_test_connection(document):
    try:
        connection = document["database"][POSTGRES_TEST_CONNECTION]
    except (KeyError, TypeError) as error:
        raise ValueError(
            f"config/setting.json must define database.{POSTGRES_TEST_CONNECTION}"
        ) from error
    if not isinstance(connection, dict):
        raise ValueError(f"database.{POSTGRES_TEST_CONNECTION} must be an object")
    if connection.get("type") != "postgres":
        raise ValueError(f"database.{POSTGRES_TEST_CONNECTION} must use type 'postgres'")
    max_connections = connection.get("max_connections")
    if type(max_connections) is not int or not 1 <= max_connections <= 256:
        raise ValueError(
            f"database.{POSTGRES_TEST_CONNECTION}.max_connections must be an explicit 1..256 integer"
        )
    return connection


def postgres_case_url(document, case_name):
    template = postgres_test_connection(document).get("url")
    if not isinstance(template, str):
        raise ValueError(f"database.{POSTGRES_TEST_CONNECTION}.url must be Text")
    parsed = urlsplit(template)
    database = parsed.path.removeprefix("/")
    if (template.count("{case}") != 1 or "{case}" not in database
            or not database or "/" in database):
        raise ValueError(
            f"database.{POSTGRES_TEST_CONNECTION}.url must contain one {{case}} placeholder "
            "in its database name"
        )
    if parsed.scheme not in ("postgres", "postgresql"):
        raise ValueError(
            f"database.{POSTGRES_TEST_CONNECTION}.url must use postgres:// or postgresql://"
        )
    return template.replace("{case}", case_name)


def postgres_database_case(output, case_name):
    run = hashlib.sha256(str(output.resolve()).encode()).hexdigest()[:12]
    return f"{run}_{case_name.replace('-', '_')}"


def load_postgres_setting(path=SETTING_PATH):
    if not path.is_file():
        return None
    try:
        document = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot load '{path}': {error}") from error
    if not isinstance(document, dict):
        raise ValueError(f"'{path}' must contain a JSON object")
    connections = document.get("database")
    if connections is None:
        return None
    if not isinstance(connections, dict):
        raise ValueError(f"'{path}' database must be an object")
    if POSTGRES_TEST_CONNECTION not in connections:
        return None
    postgres_case_url(document, "validation")
    return document


def configuration(args, postgres_setting=None):
    settings = {
        "workers": args.workers,
        "connections": args.connections,
        "pending": args.pending,
        "iterations": args.iterations,
        "channel_capacity": 64,
        "timeout_ms": 2000,
        "lifetime_ms": 3_600_000,
        "idle_sample_ms": 3000,
        "task_capacity": max(args.pending + 16, 2 * args.connections + 16),
        "max_blocking_threads": 2,
        "http2_streams": args.http2_streams,
        "http2_stream_window_bytes": args.http2_stream_window_bytes,
        "http2_connection_window_bytes": args.http2_connection_window_bytes,
        "orm_iterations": args.orm_iterations,
        "orm_rows": args.orm_rows,
        "orm_concurrency": args.orm_concurrency,
        "orm_cancel_work": args.orm_cancel_work,
        "orm_sqlite_connections": args.orm_sqlite_connections,
        "orm_postgres_connections": (
            postgres_test_connection(postgres_setting)["max_connections"]
            if postgres_setting is not None else None
        ),
    }
    if settings["task_capacity"] > 65536:
        raise ValueError("task capacity exceeds the language limit")
    return settings


def config_source(settings, fixture):
    names = CONFIG_FUNCTIONS[fixture]
    functions = {
        "lifetime": ("Int", str(settings["lifetime_ms"])),
        "idle_sample_ms": ("Int", str(settings["idle_sample_ms"])),
        "pending": ("Int", str(settings["pending"])),
        "channel_capacity": ("Int", str(settings["channel_capacity"])),
        "input": ("Bytes", f'dever.bytes.from_text({json.dumps("x" * settings["iterations"])})'),
        "pending_input": ("Bytes", f'dever.bytes.from_text({json.dumps("x" * settings["pending"])})'),
        "orm_input": ("Bytes", f'dever.bytes.from_text({json.dumps("x" * settings["orm_iterations"])})'),
        "orm_rows": ("Bytes", f'dever.bytes.from_text({json.dumps("r" * settings["orm_rows"])})'),
        "orm_concurrency": ("Int", str(settings["orm_concurrency"])),
        "orm_cancel_work": ("Int", str(settings["orm_cancel_work"])),
        "limits": ("dever.http.Limits", "dever.http.Limits {\nheader_bytes = 8192\n"
                   f'body_bytes = 65536\ntimeout_ms = {settings["timeout_ms"]}\n'
                   f'connections = {settings["connections"]}\nhttp2 = null\n}}'),
        "http2_limits": ("dever.http.Limits", "dever.http.Limits {\nheader_bytes = 8192\n"
                         f'body_bytes = 65536\ntimeout_ms = {settings["timeout_ms"]}\n'
                         f'connections = {settings["connections"]}\nhttp2 = dever.http.Http2Limits {{\n'
                         f'streams = {settings["http2_streams"]}\n'
                         f'stream_window_bytes = {settings["http2_stream_window_bytes"]}\n'
                         f'connection_window_bytes = {settings["http2_connection_window_bytes"]}\n}}\n}}'),
    }
    if "certificate" in names:
        functions["certificate"] = ("Bytes", f'dever.bytes.from_text({json.dumps((TLS / "server.pem").read_text())})')
    if "key" in names:
        functions["key"] = ("Bytes", f'dever.bytes.from_text({json.dumps((TLS / "server-key.pem").read_text())})')
    source = ""
    for name in names:
        if name == "large_body":
            continue
        kind, expression = functions[name]
        source += f"\n{name}() (result: {kind}) {{\nresult = {expression}\n}}\n"
    if "large_body" in names:
        source += '\ndouble(value: Int, text: Text) (result: Text) { result = text + text }\n'
        source += '\nlarge_body() (result: Bytes) {\nresult = dever.bytes.from_text(reduce(double, '
        source += '[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15], "x"))\n}\n'
    return source


def stage_application_command(module, capability):
    command = module / "benchmark/command"
    command.mkdir(parents=True, exist_ok=True)
    (command / "app.dever").write_text(
        f"run() (result: Bool) {{\n  result = {capability}()\n}}\n"
    )
    (command / "api.dever").write_text("cmd run = app.run\n")


def stage_benchmark_support(module, settings, fixture):
    if fixture in ("runtime", "orm"):
        bench = module / "benchmark/bench/app.dever"
        bench.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(FIXTURES / "bench.dever", bench)
    if CONFIG_FUNCTIONS[fixture]:
        config = module / "benchmark/config/app.dever"
        config.parent.mkdir(parents=True, exist_ok=True)
        config.write_text(config_source(settings, fixture))


def database_setting(driver, settings, postgres_setting=None, case_name=None):
    if driver == "sqlite":
        database = {
            "type": "sqlite",
            "path": "data/db/orm.db",
            "max_connections": settings["orm_sqlite_connections"],
            "max_page_size": max(32, min(settings["orm_rows"], 10_000)),
        }
    elif driver == "postgres":
        if postgres_setting is None or case_name is None:
            raise ValueError("PostgreSQL requires config/setting.json")
        database = deepcopy(postgres_test_connection(postgres_setting))
        database["url"] = postgres_case_url(postgres_setting, case_name)
    else:
        raise ValueError(f"unknown database driver: {driver}")
    document = ({name: deepcopy(postgres_setting[name])
                 for name in ("http", "log") if name in postgres_setting}
                if driver == "postgres" else {})
    document.setdefault("http", {})
    document.setdefault("log", {})
    document["database"] = {"default": database}
    document["auth"] = {"providers": {"benchmark": {
        "verify": "benchmark.item.verify",
        "jwtSecret": secrets.token_urlsafe(48),
        "ttlSeconds": 3600,
    }}}
    document["sites"] = {"benchmark": {"path": "", "auth": "benchmark"}}
    return document


def profile_database_setting(profile):
    if profile == "base":
        return {}
    sqlite = {
        "type": "sqlite",
        "path": "data/db/profile.db",
        "max_connections": 1,
        "max_page_size": 100,
    }
    if profile == "sqlite":
        return {"database": {"default": sqlite}}
    if profile not in ("postgres", "both"):
        raise ValueError(f"unknown runtime profile: {profile}")
    postgres = {
        "type": "postgres",
        "url": "postgres://build-only.invalid/dever_profile",
        "tls": "disabled",
        "max_connections": 1,
    }
    if profile == "postgres":
        return {"database": {"default": postgres}}
    return {"database": {"default": sqlite, "postgres_profile": postgres}}


def dependency_packages(text):
    return sorted({line.removesuffix(" (*)") for line in text.splitlines() if line})


def runtime_dependencies(cargo, features):
    command = [cargo, "tree", "--offline", "--locked", "-p", "dever-backend-bridge",
               "--no-default-features", "--prefix", "none"]
    if features:
        command.extend(("--features", ",".join(features)))
    return dependency_packages(subprocess.check_output(command, cwd=ROOT, text=True))


def prepare_native_project(destination, fixture, settings):
    module = destination / "module"
    domain = module / "benchmark" / fixture
    domain.mkdir(parents=True)
    shutil.copyfile(FIXTURES / fixture / "app.dever", domain / "app.dever")
    for role in ("port", "adapter", "api"):
        shutil.copyfile(FIXTURES / fixture / f"{role}.dever",
                        domain / f"{role}.dever")
    stage_benchmark_support(module, settings, fixture)
    config = destination / "config"
    config.mkdir()
    write_private_json(config / "setting.json",
                       native_runtime_setting(settings) if fixture == "live" else {})


def prepare_project(destination, fixture, settings, driver, postgres_setting=None):
    module = destination / "module"
    domain = module / "benchmark/item"
    domain.mkdir(parents=True)
    shutil.copyfile(FIXTURES / fixture / "app.dever", domain / "app.dever")
    shutil.copyfile(FIXTURES / fixture / "app/model/item.dever", domain / "model.dever")
    if fixture == "orm":
        shutil.copyfile(FIXTURES / fixture / "app/api.dever", domain / "api.dever")
    stage_benchmark_support(module, settings, fixture)
    stage_application_command(module, "benchmark.item.orm_idle")
    config = destination / "config"
    config.mkdir()
    write_private_json(
        config / "setting.json",
        database_setting(driver, settings, postgres_setting, "build_only"),
    )
    (destination / "data").mkdir()


def prepare_profile_project(destination, setting):
    shutil.copytree(FIXTURES / "profile", destination / "module")
    config = destination / "config"
    config.mkdir()
    write_private_json(config / "setting.json", setting)
    (destination / "data").mkdir()


def prepare_cms_project(destination, source):
    shutil.copytree(ROOT / "examples/cms" / source, destination)


def native_runtime_setting(settings):
    return {"adapter": {"benchmark.live": {"setting": {
        "limits": {
            "header_bytes": 8192,
            "body_bytes": 65536,
            "timeout_ms": settings["timeout_ms"],
            "connections": settings["connections"],
            "http2": None,
        },
        "lifetime_ms": settings["lifetime_ms"],
    }}}}


def record_binary(manifest, name, binary, started, **metadata):
    manifest["binaries"][name] = {
        **fingerprint(binary),
        "build_seconds": time.monotonic() - started,
        **metadata,
    }


def build_profile_report(args, manifest, log):
    report = {"source": "profile/benchmark/command/app.dever", "profiles": {}}
    for profile, features in RUNTIME_PROFILES.items():
        item = {"runtime_features": list(features),
                "dependencies": runtime_dependencies(args.cargo, features)}
        print(f"build profile-{profile}", flush=True)
        binary_name = f"profile-{profile}"
        binary = args.output.resolve() / binary_name
        started = time.monotonic()
        project = args.output.resolve() / "source" / f"profile-{profile}"
        setting = profile_database_setting(profile)
        prepare_profile_project(project, setting)
        subprocess.run(
            [args.compiler.resolve(), "build", project, "--output", binary],
            stdout=log,
            stderr=log,
            check=True,
        )
        record_binary(manifest, binary_name, binary, started,
                      profile=profile, fixture="profile")
        item.update({"status": "built", "binary": fingerprint(binary)})
        report["profiles"][profile] = item
    path = args.output.resolve() / "profile-report.json"
    path.write_text(json.dumps(report, indent=2) + "\n")
    manifest["files"]["profile-report.json"] = fingerprint(path)


def build(args):
    postgres_setting = load_postgres_setting() if args.selection == "all" else None
    settings = configuration(args, postgres_setting)
    destination = args.output.resolve()
    destination.mkdir(parents=True, exist_ok=False)
    compiler = args.compiler.resolve()
    cargo = shutil.which(str(args.cargo))
    if cargo is None:
        raise ValueError(f"cannot find Cargo command: {args.cargo}")
    args.cargo = Path(cargo)
    has_protocols = args.selection in ("all", "protocols")
    peers = ({name: getattr(args, name).resolve()
              for name in ("network_bench", "live_bench", "async_bench")}
             if has_protocols else {})
    required_tools = (compiler, *peers.values())
    for executable in required_tools:
        if not executable.is_file():
            raise ValueError(f"build the executable first: {executable}")
    # 在昂贵构建之前拒绝旧环境变量协议产物；这里只读配置，不启动网络。
    for name, peer in peers.items():
        if name == "async_bench":
            continue  # The finite Rust reference takes explicit positional arguments.
        with tempfile.TemporaryDirectory(prefix="dever-peer-config-") as temporary:
            executable = stage_peer(peer, Path(temporary), {"workers": 2})
            result = subprocess.run([executable, "check-config"], capture_output=True,
                                    text=True, check=True, timeout=5)
            if json.loads(result.stdout) != {"workers": 2}:
                raise ValueError(f"benchmark peer does not implement {PEER_CONFIGURATION}: {peer}")
    manifest = {
        "peer_configuration": PEER_CONFIGURATION,
        "application_configuration": APPLICATION_CONFIGURATION,
        "selection": args.selection,
        "settings": settings,
        "cargo": subprocess.check_output([cargo, "-V"], text=True),
        "compiler": fingerprint(compiler),
        "optimization": "LLVM O2 application objects, LLD static link, selected prebuilt runtime archive",
        "runtime_scheduling": "Dever uses production runtime defaults; workers/task_capacity configure Rust references only",
        "binaries": {},
        "files": {},
    }
    with (destination / "build.log").open("w") as log:
        for fixture, entries in (ENTRIES.items() if has_protocols else ()):
            source = destination / "source" / fixture
            prepare_native_project(source, fixture, settings)
            print(f"build {fixture}", flush=True)
            started = time.monotonic()
            executable = source / "app"
            subprocess.run([compiler, "build", source, "--output", executable],
                           stdout=log, stderr=log, check=True)
            for entry in entries:
                binary = destination / entry
                shutil.copy2(executable, binary)
                record_binary(manifest, entry, binary, started, profile="base", fixture=fixture,
                              command=f"benchmark.{fixture}.{entry}", shared_application=fixture)
        drivers = ()
        if args.selection == "all":
            drivers = ("sqlite", "postgres") if postgres_setting is not None else ("sqlite",)
        for driver in drivers:
            project = destination / "source" / f"orm-{driver}"
            try:
                prepare_project(project, "orm", settings, driver, postgres_setting)
                subprocess.run([compiler, "fmt", project], stdout=log, stderr=log, check=True)
                for entry in ORM_ENTRIES:
                    name = f"{driver}-{entry.removeprefix('orm_')}"
                    print(f"build {name}", flush=True)
                    binary = destination / name
                    started = time.monotonic()
                    command_entry = "benchmark.item.orm_seed" if entry == "orm_http" else f"benchmark.item.{entry}"
                    stage_application_command(project / "module", command_entry)
                    subprocess.run([compiler, "build", project, "--output", binary],
                                   stdout=log, stderr=log, check=True)
                    record_binary(manifest, name, binary, started, profile=driver, fixture="orm",
                                  entry=entry)
            finally:
                if driver == "postgres":
                    (project / "config/setting.json").unlink(missing_ok=True)
        cms_sources = {"all": ("dever",), "cms-profiles": ("dever", "md"),
                       "profiles": (), "protocols": ()}[args.selection]
        for source in cms_sources:
            cms_project = destination / "source" / f"cms-{source}"
            prepare_cms_project(cms_project, source)
            config_name = f"source/cms-{source}/config/setting.json"
            manifest["files"][config_name] = fingerprint(cms_project / "config/setting.json")
            binary_name = "cms" if source == "dever" else "cms-md"
            print(f"build cms-{source}", flush=True)
            cms_binary = destination / binary_name
            started = time.monotonic()
            subprocess.run([compiler, "build", cms_project, "--output", cms_binary],
                           stdout=log, stderr=log, check=True)
            record_binary(manifest, binary_name, cms_binary, started,
                          profile="sqlite", fixture="cms", source=source)
        if args.selection != "protocols":
            build_profile_report(args, manifest, log)
    for name, peer in peers.items():
        shutil.copy2(peer, destination / name)
        manifest["binaries"][name] = fingerprint(destination / name)
    if has_protocols:
        shutil.copytree(TLS, destination / "tls")
        for path in sorted((destination / "tls").rglob("*")):
            if path.is_file():
                name = path.relative_to(destination).as_posix()
                manifest["files"][name] = fingerprint(path)
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"artifacts: {destination}", flush=True)

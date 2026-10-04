"""显式 ignored 验收与现有长测运行器的 Linux gate 接线。"""

import json
from pathlib import Path
import re
from urllib.parse import parse_qsl, unquote, urlsplit

from linux_gate import (ROOT, MissingInput, absolute_file, run_command,
                        target_command, workspace_targets, remove_executables, input_fingerprint)
from settings import write_private_json
from process import resource_trend
from cms import validate_read_descriptor_recovery


def execute(command, config, directory, result, *, single_harness=False):
    record, content = run_command(command, directory, len(result["commands"]),
                                  config.get("command_timeout_seconds", 14400),
                                  single_harness=single_harness)
    result["commands"].append(record)
    return record, content


def classify_cases(names, manifest, target):
    by_name = {}
    for row in manifest:
        key = (row["package"], row["target"], row["name"])
        if key in by_name:
            raise RuntimeError(f"duplicate ignored classification: {key}")
        by_name[key] = row
    classified = []
    for name in names:
        leaf = name.rsplit("::", 1)[-1]
        key = (target["package"], target["target"], leaf)
        if key not in by_name:
            raise RuntimeError(f"unclassified ignored test: {name}")
        classified.append({**by_name[key], "exact": name})
    if len({case["name"] for case in classified}) != len(classified):
        raise RuntimeError("same ignored leaf appears in multiple modules; classify by a distinct source name")
    return classified


def audit_classification():
    manifest = json.loads((ROOT / "test/ci/ignored.json").read_text())
    stages = {"probe", "author", "native", "postgres", "sandbox", "bounded-perf"}
    if any(row["stage"] not in stages for row in manifest):
        raise RuntimeError("ignored classification contains an unknown execution stage")
    expected = {(row["source"], row["name"], row["reason"]) for row in manifest}
    actual = set()
    pattern = re.compile(r'#\[ignore(?:\s*=\s*"([^"]*)")?\]\s*(?:#\[[^\]]*\]\s*)*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)')
    for directory in ("test", "crates"):
        for path in (ROOT / directory).rglob("*.rs"):
            for match in pattern.finditer(path.read_text()):
                actual.add((str(path.relative_to(ROOT)), match[2], match[1] or ""))
    if actual != expected:
        raise RuntimeError(f"ignored classification drift: new={sorted(actual - expected)}, removed={sorted(expected - actual)}")
    return manifest


def test_executable(content, target):
    artifacts = []
    for line in content.splitlines():
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        if (message.get("reason") == "compiler-artifact" and message.get("profile", {}).get("test")
                and message.get("target", {}).get("name") == target["target"] and message.get("executable")):
            artifacts.append(message["executable"])
    if len(set(artifacts)) != 1:
        raise RuntimeError(f"expected one libtest executable for {target}")
    return artifacts[0]


def inventory(config, directory, result):
    manifest = audit_classification()
    targets, target_directory = workspace_targets(config, directory, result, True)
    cases = []
    for target in targets:
        if target["selector"] == ["--doc"]:
            continue
        _, content = execute([*target_command(config, target), "--no-run", "--message-format=json"], config, directory, result)
        executable = test_executable(content, target)
        _, listing = execute([executable, "--ignored", "--list", "--format", "terse"], config, directory, result)
        names = [line.removesuffix(": test") for line in listing.splitlines() if line.endswith(": test")]
        cases.extend({**case, "target": target, "target_directory": str(target_directory)}
                     for case in classify_cases(names, manifest, target))
        result.setdefault("removed_test_executables", []).extend(
            remove_executables(content, target_directory, config.get("remove_test_executables", False)))
        write_private_json(directory / "inventory.json", cases, replace=(directory / "inventory.json").exists())
    missing = {(row["package"], row["target"], row["name"]) for row in manifest} - {
        (case["target"]["package"], case["target"]["target"], case["name"]) for case in cases}
    if missing:
        raise RuntimeError(f"ignored source cases missing from Linux all-feature libtest inventory: {sorted(missing)}")
    result["classified_cases"] = len(cases)
    result["internal_probes"] = [{"target": case["target"], "exact": case["exact"],
                                  "status": "parent-only", "reason": case["reason"]}
                                 for case in cases if case["stage"] == "probe"]


def current_inventory(document, config, result):
    paths = sorted((Path(config.get("output", ROOT / "target/linux-ci")) / "inventory").glob("*/result.json"))
    if paths:
        path = paths[-1]
        record = json.loads(path.read_text())
        if (record.get("source"), record.get("inputs"), record.get("status")) == (result["source"], input_fingerprint(document, config, "inventory"), "passed"):
            return json.loads((path.parent / "inventory.json").read_text())
    raise MissingInput("run inventory for the current source and input fingerprints first")


def validate_postgres(document):
    setting = document.get("database", {}).get("postgres_test", {})
    url = urlsplit(setting.get("url", ""))
    database = unquote(url.path.removeprefix("/"))
    if (setting.get("type") != "postgres" or url.scheme not in ("postgres", "postgresql")
            or url.hostname not in ("127.0.0.1", "::1") or url.port is None
            or database.count("{case}") != 1
            or any(key.lower() in ("dbname", "host", "hostaddr", "port", "service") for key, _ in parse_qsl(url.query))):
        raise MissingInput("database.postgres_test must select the explicitly owned loopback cluster and one {case} database template")


def require_inputs(stage, config):
    required = config.get("required_inputs", {}).get(stage)
    if not required:
        raise MissingInput(f"performance.linux_ci.required_inputs.{stage} must declare the prepared author/fixture input paths")
    declared = [Path(value).resolve() for value in config.get("inputs", [])]
    for value in required:
        path = Path(value)
        if not path.is_absolute() or not path.exists():
            raise MissingInput(f"missing {stage} input: {value}")
        if not any(path.resolve() == root or path.resolve().is_relative_to(root) for root in declared):
            raise MissingInput(f"{stage} input is not fingerprinted by performance.linux_ci.inputs: {value}")


def ignored_stage(stage, document, config, directory, result):
    require_inputs(stage, config)
    if stage == "postgres":
        validate_postgres(document)
        if not config.get("owned_postgres"):
            raise MissingInput("performance.linux_ci.owned_postgres must record the owned cluster data_directory and port")
        owned = config["owned_postgres"]
        if urlsplit(document["database"]["postgres_test"]["url"]).port != owned.get("port"):
            raise MissingInput("PostgreSQL URL does not match performance.linux_ci.owned_postgres.port")
        if not Path(owned.get("data_directory", "")).is_absolute():
            raise MissingInput("owned_postgres.data_directory must be absolute")
    if stage == "author" and config.get("author_network") is not True:
        raise MissingInput("author stage fetches fixed upstream inputs; explicitly set performance.linux_ci.author_network=true")
    if stage == "author" and not config.get("author_outputs"):
        raise MissingInput("declare performance.linux_ci.author_outputs separately from immutable raw author inputs")
    cases = [case for case in current_inventory(document, config, result) if case["stage"] == stage]
    if not cases:
        raise RuntimeError(f"no classified cases for {stage}")
    # 固定作者先准备工具，再执行真实源码包构建；其他用例按 target 复用链接产物。
    cases.sort(key=lambda case: (0 if case["name"].startswith("prepare_") else 1,
                                 case["target"]["package"], case["target"]["target"], case["exact"]))
    target_key = None
    build_content = None
    target_directory = ROOT / "target"
    for case in cases:
        target = case["target"]
        target_directory = Path(case["target_directory"])
        result["target_directory"] = str(target_directory)
        key = json.dumps(target, sort_keys=True)
        if key != target_key:
            if build_content is not None:
                result.setdefault("removed_test_executables", []).extend(
                    remove_executables(build_content, target_directory, config.get("remove_test_executables", False)))
            _, build_content = execute([*target_command(config, target), "--no-run", "--message-format=json"], config, directory, result)
            executable = test_executable(build_content, target)
            target_key = key
        record, _ = execute([executable, "--ignored", "--exact", case["exact"], "--test-threads=1", "--nocapture"],
                            config, directory, result, single_harness=True)
        if record.get("rust_tests") != {"passed": 1, "failed": 0, "ignored": 0}:
            raise RuntimeError(f"exact ignored selection did not execute one case: {case['exact']}")
        result.setdefault("cases", []).append({"exact": case["exact"], "target": target, "status": "passed"})
    result.setdefault("removed_test_executables", []).extend(
        remove_executables(build_content, target_directory, config.get("remove_test_executables", False)))


def validate_budget(config):
    parent = Path(config.get("cgroup_parent", ""))
    if not parent.is_absolute() or not (parent / "cgroup.controllers").is_file():
        raise MissingInput("performance.linux_ci.cgroup_parent must select the owned empty cgroup v2 parent")
    if (parent / "cgroup.procs").read_text().strip():
        raise MissingInput("CI runner must be in a control child, not the cgroup parent")
    if not {"cpu", "memory"} <= set((parent / "cgroup.subtree_control").read_text().split()):
        raise MissingInput("owned parent requires enabled memory/cpu child controllers")
    quota, period = (parent / "cpu.max").read_text().split()
    memory = (parent / "memory.max").read_text().strip()
    if quota == "max" or int(quota) / int(period) > 2 or memory == "max" or int(memory) > 4 * 1024 ** 3:
        raise MissingInput("owned parent must bound total CPU <=2 and memory <=4GiB")
    if (parent / "memory.swap.max").read_text().strip() != "0":
        raise MissingInput("owned parent must disable swap")
    membership = Path("/proc/self/cgroup").read_text().strip().split("::", 1)[1]
    current = Path("/sys/fs/cgroup") / membership.lstrip("/")
    if current == parent or not current.is_relative_to(parent):
        raise MissingInput("CI runner is outside the configured owned parent control subgroup")
    return parent


def validate_soak(report, suite, seconds, growth_limit, memory_mib):
    if report.get("status") not in ("passed", "complete"):
        raise RuntimeError("soak runner did not complete")
    cases = list(report["cases"].values()) if suite == "cms" else report["cases"]
    expected = 3 if suite == "live" else 2
    if len(cases) != expected:
        raise RuntimeError(f"expected {expected} {suite} cases, received {len(cases)}")
    if suite == "cms":
        coverage = set(report["cases"])
        required = {"dever", "md"}
    else:
        coverage = {case["protocol" if suite == "live" else "transport"] for case in cases}
        required = {"tcp", "ws", "sse"} if suite == "live" else {"http", "https"}
        if any(case["suite"] != suite or (suite != "live" and case["implementation"] != "dever") for case in cases):
            raise RuntimeError("soak report contains a different suite or implementation")
    if coverage != required:
        raise RuntimeError("soak report does not cover every required source or transport")
    for case in cases:
        budget = case.get("cgroup") if suite == "cms" else case.get("budget_after_stop")
        if budget is None or any(budget["events"][key] for key in ("oom", "oom_kill")):
            raise RuntimeError("missing memory-budget evidence or nonzero OOM")
        if budget["max"] != str(memory_mib * 1024 ** 2):
            raise RuntimeError("soak used a different memory limit from the selected stage")
        if suite == "cms":
            load = case["concurrent_reads"]
            if load["elapsed_seconds"] < seconds or load["errors"] or load["succeeded"] == 0:
                raise RuntimeError("CMS duration, business load or FD recovery failed")
            validate_read_descriptor_recovery(load)
            trend = load["trend"]
        elif suite == "live":
            if (case["result"]["elapsed_ms"] < seconds * 1000 or case["result"]["errors"]
                    or any(case["fd_delta_after_cycles"])):
                raise RuntimeError("live duration, protocol or FD recovery failed")
            samples = [phase["last"] for phase in case["phases"] if phase["phase"] == "disconnected"]
            trend = resource_trend(samples)
        else:
            loads = [case["load"]] if suite == "http2" else [cycle["load"] for cycle in case["cycles"]]
            disconnected = [case["disconnected"]] if suite == "http2" else [cycle["disconnected"] for cycle in case["cycles"]]
            if any(sample["fd_delta"] != 0 for sample in disconnected):
                raise RuntimeError("HTTP/2 did not restore file descriptors after disconnect")
            if sum(load["elapsed_seconds"] for load in loads) < seconds:
                raise RuntimeError("HTTP/2 measured load is shorter than the required duration")
            for load in loads:
                if load["errors"] or load["succeeded"] <= 0 or load["succeeded"] < .99 * load["scheduled"]:
                    raise RuntimeError("HTTP/2 did not achieve 99% scheduled successful replies with zero errors")
            trend = resource_trend(case["raw"])
        if trend is None or trend["rss_late_growth_bytes"] > growth_limit:
            raise RuntimeError("late RSS growth exceeds the configured budget; inspect raw resource samples")
        case["gate_resource_trend"] = trend


def soak_stage(stage, config, directory, result):
    suite, memory = stage.rsplit("-", 1)
    suite = {"recovery": "http2-recovery"}.get(suite, suite)
    require_inputs("soak", config)
    parent = validate_budget(config)
    settings = config.get("soak", {})
    seconds = settings.get("seconds", 1800)
    if not 1800 <= seconds <= 2700:
        raise ValueError("performance.linux_ci.soak.seconds must be 1800..2700")
    python = str(absolute_file(config["tools"].get("python"), "performance.linux_ci.tools.python"))
    output = directory / "measurement"
    if suite == "cms":
        manifest = str(absolute_file(settings.get("cms_manifest"), "performance.linux_ci.soak.cms_manifest"))
        command = [python, "-B", "test/performance/cms.py", "run", "--manifest", manifest,
                   "--concurrency", str(settings.get("cms_concurrency", 16)), "--read-seconds", str(seconds)]
    else:
        artifacts = Path(settings.get("artifacts", ""))
        if not artifacts.is_absolute() or not (artifacts / "manifest.json").is_file():
            raise MissingInput("performance.linux_ci.soak.artifacts requires prepared protocol artifacts")
        command = [python, "-B", "test/performance/run.py", "run", "--artifacts", str(artifacts),
                   "--suite", suite, "--implementations", "dever", "--repeats", "1", "--interval", "0.1"]
        if suite == "http2":
            command += ["--duration", str(seconds), "--warmup", "5", "--paths", "/json",
                        "--rates", str(settings.get("http2_rate", 5000)), "--http2-connections", "4"]
        else:
            if seconds % 30:
                raise ValueError("cyclic soak seconds must be divisible by 30")
            command += ["--duration", "30", "--cycles", str(seconds // 30)]
            command += (["--connection-tiers", "32"] if suite == "live" else
                        ["--recovery-rate", str(settings.get("http2_rate", 5000)),
                         "--recovery-path", "/json", "--recovery-connections", "4"])
        if "server_cpu" in settings:
            command += ["--server-cpu", str(settings["server_cpu"]), "--client-cpus", str(settings["client_cpu"])]
    command += ["--output", str(output), "--memory-mib", memory, "--cgroup-parent", str(parent)]
    execute(command, config, directory, result)
    report = json.loads((output / "report.json").read_text())
    validate_soak(report, suite, seconds, settings.get("rss_late_growth_mib", 8) * 1024 ** 2, int(memory))
    write_private_json(directory / "validated-report.json", report)
    result["measurement"] = str(output / "report.json")
    result["coverage"] = "CMS dever/md; HTTP/2 h2c/TLS JSON; recovery h2c/TLS; live TCP/WS/SSE, each selected stage only"


def run_stage(stage, document, config, directory, result):
    if stage == "inventory":
        inventory(config, directory, result)
    elif stage in ("author", "native", "postgres", "sandbox", "bounded-perf"):
        ignored_stage(stage, document, config, directory, result)
    else:
        soak_stage(stage, config, directory, result)

"""Linux 质量门：串行复用现有检查，按源码和输入摘要保存阶段证据。"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.append(str(ROOT / "test/performance"))
sys.modules["linux_gate"] = sys.modules[__name__]
from settings import configured_peer, write_private_json

STAGES = ("static", "default", "all-features", "features", "python", "sdk",
          "inventory", "author", "native", "postgres", "sandbox", "bounded-perf",
          "cms-64", "cms-128", "http2-64", "http2-128", "recovery-64", "recovery-128",
          "live-64", "live-128")
SOURCE_ROOTS = ("crates", "test", "sdk", "library", "examples", ".cargo")


class MissingInput(Exception):
    pass


def digest_file(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def source_fingerprint(root=ROOT):
    paths = [root / name for name in ("Cargo.toml", "Cargo.lock")]
    for name in SOURCE_ROOTS:
        directory = root / name
        if directory.exists():
            paths.extend(path for path in directory.rglob("*") if path.is_file()
                         and not {"__pycache__", "target", ".git"}.intersection(path.parts))
    digest = hashlib.sha256()
    for path in sorted(paths):
        digest.update(str(path.relative_to(root)).encode() + b"\0")
        digest.update(digest_file(path).encode() + b"\0")
    return digest.hexdigest()


def absolute_file(value, name):
    path = Path(value or "")
    if not path.is_absolute() or not path.is_file():
        raise MissingInput(f"{name} requires an existing absolute file path")
    return path


def load_config():
    path = ROOT / "config/setting.json"
    if not path.is_file():
        raise MissingInput("create config/setting.json with performance.linux_ci (see test/ci/README.md)")
    document = json.loads(path.read_text())
    performance = document.get("performance") if isinstance(document, dict) else None
    config = performance.get("linux_ci") if isinstance(performance, dict) else None
    if not isinstance(config, dict):
        raise MissingInput("config/setting.json must define performance.linux_ci")
    return document, config


def input_fingerprint(document, config, stage=None):
    inputs = {}
    for name, value in config.get("tools", {}).items():
        path = Path(value)
        if not path.is_absolute():
            raise ValueError("performance.linux_ci.tools must use absolute paths")
        inputs[f"tool:{name}:{value}"] = digest_file(path) if path.is_file() else "missing"
    paths = set(config.get("inputs", []))
    paths.update(str(configured_peer(name, ROOT / "config/setting.json"))
                 for name in ("network_peer", "live_peer") if name in document.get("performance", {}))
    soak = config.get("soak", {})
    if soak.get("artifacts"):
        paths.add(soak["artifacts"])
    if soak.get("cms_manifest"):
        manifest = Path(soak["cms_manifest"])
        paths.add(str(manifest))
        if manifest.is_file():
            for case in json.loads(manifest.read_text()).get("cases", {}).values():
                paths.update(case[key] for key in ("executable", "config"))
    for value in sorted(paths):
        path = Path(value)
        if not path.is_absolute():
            raise ValueError("performance.linux_ci.inputs must use absolute paths")
        if stage == "inventory":
            continue  # 清单依赖当前源码和工具，不消费作者资产。
        outputs = [Path(value) for value in config.get("author_outputs", [])] if stage == "author" else []
        if any(path == output or path.is_relative_to(output) for output in outputs):
            continue
        if path.is_dir():
            for child in sorted(path.rglob("*")):
                if child.is_file() and not any(child == output or child.is_relative_to(output) for output in outputs):
                    inputs[str(child)] = digest_file(child)
        else:
            inputs[value] = digest_file(path) if path.is_file() else "missing"
    # 保存摘要而非数据库密码/令牌；配置任何变化都使旧证据失效。
    return hashlib.sha256(json.dumps([document, inputs], sort_keys=True).encode()).hexdigest()


def command_plan(stage, config):
    tools = config.get("tools", {})
    cargo = str(absolute_file(tools.get("cargo"), "performance.linux_ci.tools.cargo"))
    common = ["--offline", "--locked", "-j1", *cargo_config(config)]
    test = [cargo, "test", *common, "--workspace"]
    if stage == "static":
        fmt = str(absolute_file(tools.get("cargo_fmt"), "performance.linux_ci.tools.cargo_fmt"))
        clippy = str(absolute_file(tools.get("cargo_clippy"), "performance.linux_ci.tools.cargo_clippy"))
        return [[fmt, "fmt", "--all", "--check"],
                *[[clippy, "clippy", *common, "--workspace", "--all-targets", *features,
                   "--", "-D", "warnings"] for features in ([], ["--all-features"])]]
    if stage in ("default", "all-features"):
        return [[*test, *(["--all-features"] if stage == "all-features" else []),
                 "--", "--test-threads=1"]]
    if stage == "features":
        profiles = {
            "dever-runtime": ["", "auth-crypto", "crypto", "wire", "external", "api", "database", "sqlite", "postgres", "sqlite,postgres"],
            "dever-backend-bridge": ["", "embedded", "runtime-abi", "runtime-database", "runtime-api", "runtime-external", "runtime-sqlite", "runtime-postgres", "runtime-sqlite,runtime-postgres"],
        }
        return [[cargo, "check", *common, "-p", package, "--lib", "--no-default-features",
                 *(["--features", features] if features else [])]
                for package, combinations in profiles.items() for features in combinations] + [
                    [cargo, "tree", "--offline", "--locked", "-p", "dever-cli", "--edges", "features"]]
    python = str(absolute_file(tools.get("python"), "performance.linux_ci.tools.python"))
    if stage == "python":
        return [[python, "-B", "-m", "unittest", "discover", "-s", directory, "-p", "test_*.py", "-v"]
                for directory in ("test/ci", "test/performance", "test/ecosystem-release")]
    if stage == "sdk":
        return [[python, "-B", "test/external-sdk/check.py", "--python", python,
                 "--node", str(absolute_file(tools.get("node"), "performance.linux_ci.tools.node")),
                 "--go", str(absolute_file(tools.get("go"), "performance.linux_ci.tools.go"))]]
    raise ValueError(f"stage has a dedicated runner: {stage}")


def cargo_config(config):
    return [value for override in config.get("cargo_config", []) for value in ("--config", override)]


def workspace_targets(config, directory, result, all_features):
    cargo = str(absolute_file(config["tools"].get("cargo"), "performance.linux_ci.tools.cargo"))
    command = [cargo, "metadata", "--offline", "--locked", "--format-version", "1", *cargo_config(config)]
    if all_features:
        command.append("--all-features")
    record, content = run_command(command, directory, len(result["commands"]), 120)
    result["commands"].append(record)
    metadata = next(json.loads(line) for line in content.splitlines() if line.startswith('{"packages":'))
    active = {node["id"]: set(node["features"]) for node in metadata["resolve"]["nodes"]}
    targets = []
    result["target_directory"] = metadata["target_directory"]
    for package in metadata["packages"]:
        if package["id"] not in metadata["workspace_members"]:
            continue
        for target in package["targets"]:
            required = set(target.get("required-features", []))
            if not required <= active[package["id"]]:
                continue
            kind = target["kind"][0]
            if target["test"] and kind in ("lib", "rlib", "staticlib", "bin", "test"):
                selector = ["--lib"] if kind in ("lib", "rlib", "staticlib") else ["--" + kind, target["name"]]
                targets.append({"package": package["name"], "target": target["name"], "selector": selector,
                                "features": sorted(active[package["id"]])})
            if target["doctest"] and kind in ("lib", "rlib", "staticlib"):
                targets.append({"package": package["name"], "target": target["name"], "selector": ["--doc"],
                                "features": sorted(active[package["id"]])})
    return targets, Path(metadata["target_directory"])


def target_command(config, target, all_features=True):
    features = (["--all-features"] if all_features else
                ["--features", ",".join(target["features"])] if target.get("features") else [])
    return [config["tools"]["cargo"], "test", "--offline", "--locked", "-j1", *cargo_config(config),
            "-p", target["package"], *target["selector"], *features]


def remove_executables(content, target_directory, enabled):
    if not enabled:
        return []
    removed = []
    for line in content.splitlines():
        if not line.startswith("{"):
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (message.get("reason") == "compiler-artifact" and message.get("profile", {}).get("test")
                and message.get("executable")):
            path = Path(message["executable"]).resolve()
            if not path.is_relative_to(target_directory.resolve()) or path.parent.name != "deps":
                raise RuntimeError(f"refusing to remove a non-test output: {path}")
            if path.exists():
                path.unlink()
                removed.append(str(path))
    return removed


def cleanup_test_executables(directory, config, result):
    if not config.get("remove_test_executables", False) or "target_directory" not in result:
        return
    for command_file in sorted(directory.glob("*.command.json")):
        command = json.loads(command_file.read_text())
        arguments = command["command"]
        if arguments[0] != config["tools"]["cargo"] or "--message-format=json" not in arguments:
            continue
        log = Path(command["log"])
        result.setdefault("removed_test_executables", []).extend(
            remove_executables(log.read_text(errors="replace"), Path(result["target_directory"]), True))


def run_workspace(stage, config, directory, result):
    all_features = stage == "all-features"
    targets, target_directory = workspace_targets(config, directory, result, all_features)
    result["targets"] = targets
    passed = 0
    for target in targets:
        command = [*target_command(config, target, all_features), "--message-format=json", "--", "--test-threads=1"]
        print(f"{stage}: {target['package']} {' '.join(target['selector'])}", flush=True)
        record, content = run_command(command, directory, len(result["commands"]), config.get("command_timeout_seconds", 14400))
        result["commands"].append(record)
        passed += record.get("rust_tests", {}).get("passed", 0)
        result.setdefault("removed_test_executables", []).extend(
            remove_executables(content, target_directory, config.get("remove_test_executables", False)))
    if not targets or not passed:
        raise RuntimeError("workspace target matrix executed no Rust tests")
    result["passed_tests"] = passed


def run_command(command, directory, number, timeout, *, single_harness=False):
    path = directory / f"{number:03d}.log"
    started = time.time()
    record = {"command": command, "log": str(path), "started": started}
    write_private_json(directory / f"{number:03d}.command.json", record)
    failure = None
    child = None
    try:
        with path.open("x") as output:
            child = subprocess.Popen(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT,
                                     start_new_session=True)
            try:
                child.wait(timeout=timeout)
            except BaseException:
                try:
                    # Python 测量器以 KeyboardInterrupt 执行其 finally，回收独立进程组和 cgroup。
                    os.killpg(child.pid, signal.SIGINT)
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
                except ProcessLookupError:
                    child.wait()
                raise
    except BaseException as error:
        failure = error
        record["error"] = str(error) or type(error).__name__
    record["exit_code"] = child.returncode if child is not None else None
    record["elapsed_seconds"] = time.time() - started
    content = path.read_text(errors="replace")
    counts = re.findall(r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored", content)
    if single_harness:
        # 直接运行一个 libtest 时，最后一条是父 harness；前面的可能是其子进程探针。
        counts = counts[-1:]
    if counts:
        record["rust_tests"] = {name: sum(int(row[index]) for row in counts)
                                for index, name in enumerate(("passed", "failed", "ignored"))}
    record["python_tests"] = sum(map(int, re.findall(r"Ran (\d+) tests? in", content)))
    write_private_json(directory / f"{number:03d}.command.json", record, replace=True)
    if failure is not None:
        raise failure
    if record["exit_code"]:
        raise RuntimeError(f"command exited {record['exit_code']}: {path}")
    if re.search(r"OK \(skipped=|: unavailable", content):
        raise MissingInput(f"command skipped required verification: {path}")
    return record, content


def check_output(stage, record, content):
    if stage in ("default", "all-features") and record.get("rust_tests", {}).get("passed", 0) == 0:
        raise RuntimeError("workspace command did not execute any Rust tests")
    if stage == "python" and record["python_tests"] == 0:
        raise RuntimeError("Python discovery executed no tests")
    if stage == "sdk" and any(f"{language}: handshake, health, result, business error, cancel, shutdown and rejection passed"
                              not in content for language in ("Python", "JavaScript", "Go")):
        raise RuntimeError("all three SDK acceptance results are required")
    if stage == "features" and "dever-core feature \"reference\"" in content:
        raise RuntimeError("production CLI depends on the reference feature")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("plan", "run", "summary"))
    parser.add_argument("--stage", choices=STAGES)
    args = parser.parse_args()
    document, config = load_config()
    output = Path(config.get("output", ROOT / "target/linux-ci"))
    if not output.is_absolute():
        raise ValueError("performance.linux_ci.output must be absolute")
    source = source_fingerprint()
    if args.action == "summary":
        results = {}
        for stage in STAGES:
            candidates = sorted((output / stage).glob("*/result.json"))
            record = json.loads(candidates[-1].read_text()) if candidates else {}
            stage_inputs = input_fingerprint(document, config, stage)
            current = (record.get("source") == source and record.get("inputs") == stage_inputs
                       and (stage != "author" or record.get("outputs") == input_fingerprint(document, config)))
            results[stage] = record["status"] if current else "missing-current-evidence"
        print(json.dumps(results, indent=2))
        return 0 if all(status == "passed" for status in results.values()) else 1
    if args.action == "plan":
        inputs = input_fingerprint(document, config, args.stage)
        print(json.dumps({"stages": STAGES, "source": source, "inputs": inputs,
                          "commands": command_plan(args.stage, config) if args.stage in STAGES[:6] else None}, indent=2))
        return 0
    if args.stage is None:
        parser.error("run requires --stage (all stages are serialized by the caller)")
    directory = output / args.stage / f"{time.time_ns()}"
    directory.mkdir(parents=True, exist_ok=False)
    result = {"stage": args.stage, "source": source, "inputs": None, "status": "running", "commands": []}
    write_private_json(directory / "result.json", result)
    try:
        try:
            inputs = input_fingerprint(document, config, args.stage)
        except (ValueError, OSError) as error:
            raise MissingInput(str(error)) from error
        result["inputs"] = inputs
        write_private_json(directory / "result.json", result, replace=True)
        from acceptance import validate_budget
        validate_budget(config)
        if args.stage in ("default", "all-features"):
            run_workspace(args.stage, config, directory, result)
        elif args.stage in STAGES[:6]:
            for number, command in enumerate(command_plan(args.stage, config)):
                print(f"{args.stage}: {' '.join(command)}", flush=True)
                record, content = run_command(command, directory, number, config.get("command_timeout_seconds", 14400))
                result["commands"].append(record)
                check_output(args.stage, record, content)
        else:
            from acceptance import run_stage
            run_stage(args.stage, document, config, directory, result)
        current_document, current_config = load_config()
        final_inputs = input_fingerprint(current_document, current_config, args.stage)
        if args.stage == "author":
            result["outputs"] = input_fingerprint(current_document, current_config)
        if source_fingerprint() != source or final_inputs != inputs:
            raise RuntimeError("source or declared inputs changed while the stage ran; evidence is stale")
        result["status"] = "passed"
    except MissingInput as error:
        result.update(status="blocked", error=str(error))
    except (Exception, KeyboardInterrupt) as error:
        result.update(status="failed", error=str(error) or type(error).__name__)
    finally:
        try:
            cleanup_test_executables(directory, config, result)
        except Exception as error:
            result.update(status="failed", cleanup_error=str(error))
        result["commands"] = [json.loads(path.read_text()) for path in sorted(directory.glob("*.command.json"))]
        write_private_json(directory / "result.json", result, replace=True)
        print(json.dumps(result, indent=2))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (MissingInput, ValueError, OSError) as error:
        print(f"blocked: {error}", file=sys.stderr)
        sys.exit(2)

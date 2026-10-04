#!/usr/bin/env python3
"""显式构建/运行独立二进制基准；不属于 cargo test 默认目标。"""

import argparse
from contextlib import contextmanager
import http.client
from itertools import product
import json
import math
import os
from pathlib import Path
import platform
import select
import shutil
import socket
import statistics
import subprocess
import time

from build import (APPLICATION_CONFIGURATION, BENCHMARK_COMMAND, ENTRIES, ROOT, build, database_setting, fingerprint, load_postgres_setting,
                   native_runtime_setting, postgres_case_url, postgres_database_case,
                   write_private_json)
from live import lifecycle_seconds, load_live
from process import Budget, Process, observe, summarize
from settings import PEER_CONFIGURATION

HTTP_IMPLEMENTATIONS = ("dever", "runtime", "hyper")
HTTP2_DISCONNECT_SECONDS = 0.3
HTTP_BODY_BYTES = {"/plain": 5, "/json": 11, "/bytes": 65_536}
ORM_ENTRIES = ("idle", "crud", "list", "cursor", "stream", "pool", "http")
ORM_COUNT_PATH = "/benchmark/item/count"
ORM_CANCEL_PATH = "/benchmark/item/cancel"


def application_command(executable):
    return [executable, BENCHMARK_COMMAND, "{}"]


def fixture_command(executable, entry):
    fixture = next(name for name, entries in ENTRIES.items() if entry in entries)
    return [executable, f"benchmark.{fixture}.{entry}", "{}"]


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("must be finite and positive")
    return number


def positive_int(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return number


def csv_positive_ints(value, name):
    parts = value.split(",")
    if any(not part for part in parts):
        raise ValueError(f"{name} must contain unique positive integers")
    values = tuple(positive_int(part) for part in parts)
    if len(values) != len(set(values)):
        raise ValueError(f"{name} must contain unique positive integers")
    return values


def parse_client_cpus(value):
    parts = value.split(",")
    if any(not part for part in parts):
        raise ValueError("client-cpus must contain unique nonnegative integers")
    try:
        cpus = tuple(int(part) for part in parts)
    except ValueError as error:
        raise ValueError("client-cpus must contain unique nonnegative integers") from error
    if any(cpu < 0 for cpu in cpus) or len(cpus) != len(set(cpus)):
        raise ValueError("client-cpus must contain unique nonnegative integers")
    return cpus


def validate_cpu_placement(server_cpu, client_cpus, allowed):
    if server_cpu is not None and server_cpu not in allowed:
        raise ValueError(f"CPU {server_cpu} is outside allowed CPUs {sorted(allowed)}")
    if client_cpus is not None:
        for cpu in client_cpus:
            if cpu not in allowed:
                raise ValueError(f"CPU {cpu} is outside allowed CPUs {sorted(allowed)}")
        if server_cpu in client_cpus:
            raise ValueError("server and client must use different CPUs")


def parse_implementations(value):
    implementations = tuple(value.split(","))
    if (any(not part for part in implementations)
            or len(implementations) != len(set(implementations))
            or any(part not in HTTP_IMPLEMENTATIONS for part in implementations)):
        raise ValueError("implementations must contain unique dever, runtime or hyper values")
    return implementations


def implementation_order(implementations, repeat):
    return implementations if repeat % 2 == 0 else tuple(reversed(implementations))


def parser():
    cli = argparse.ArgumentParser(description=__doc__)
    commands = cli.add_subparsers(dest="action", required=True)
    prepare = commands.add_parser("build", help="build release native fixtures; no measurements")
    prepare.add_argument("--output", type=Path, required=True)
    prepare.add_argument("--selection", choices=("all", "cms-profiles", "profiles", "protocols"), default="all",
                         help="build all fixtures, CMS/profiles, profiles, or runtime/network protocols")
    prepare.add_argument("--compiler", type=Path, default=ROOT / "target/debug/dever")
    prepare.add_argument("--cargo", default="cargo")
    for name in ("network_bench", "live_bench", "async_bench"):
        prepare.add_argument("--" + name.replace("_", "-"), type=Path,
                             default=ROOT / "target/native-runtime/release/examples" / name)
    prepare.add_argument("--workers", type=positive_int, default=1)
    prepare.add_argument("--connections", type=positive_int, default=32)
    prepare.add_argument("--http2-streams", type=positive_int, default=16)
    prepare.add_argument("--http2-stream-window-bytes", type=positive_int, default=65_535)
    prepare.add_argument("--http2-connection-window-bytes", type=positive_int, default=262_144)
    prepare.add_argument("--pending", type=positive_int, default=256)
    prepare.add_argument("--iterations", type=positive_int, default=16384)
    prepare.add_argument("--orm-iterations", type=positive_int, default=128)
    prepare.add_argument("--orm-rows", type=positive_int, default=128)
    prepare.add_argument("--orm-concurrency", type=positive_int, default=8)
    prepare.add_argument("--orm-cancel-work", type=positive_int, default=1_000_000)
    prepare.add_argument("--orm-sqlite-connections", type=positive_int, default=1)
    measure = commands.add_parser("run", help="measure already-built binaries only")
    measure.add_argument("--artifacts", type=Path, required=True)
    measure.add_argument("--output", type=Path, required=True)
    measure.add_argument("--suite", choices=("all", "memory", "async", "http", "live", "http2", "http2-recovery", "orm", "cms"), default="all")
    measure.add_argument("--duration", type=positive, default=10)
    measure.add_argument("--warmup", type=positive, default=2)
    measure.add_argument("--idle-seconds", type=positive, default=1)
    measure.add_argument("--interval", type=positive, default=0.05)
    measure.add_argument("--repeats", type=positive_int, default=3)
    measure.add_argument("--rates", default="100,1000")
    measure.add_argument("--paths", default="/plain,/json,/bytes")
    measure.add_argument("--concurrency", type=positive_int, default=16)
    measure.add_argument("--connection-mode", choices=("reuse", "new", "both"), default="reuse")
    measure.add_argument("--implementations", default=",".join(HTTP_IMPLEMENTATIONS))
    measure.add_argument("--http2-connections", default="1,4")
    measure.add_argument("--recovery-connections", type=positive_int, default=1)
    measure.add_argument("--recovery-rate", type=positive_int, default=1_000)
    measure.add_argument("--recovery-path", choices=("/plain", "/json", "/bytes"), default="/plain")
    measure.add_argument("--connection-tiers", default="32")
    measure.add_argument("--cycles", type=positive_int, default=3)
    measure.add_argument("--server-cpu", type=int)
    measure.add_argument("--client-cpus", type=parse_client_cpus)
    measure.add_argument("--client-workers", type=positive_int, default=1)
    measure.add_argument("--cgroup-parent", type=Path)
    measure.add_argument("--memory-mib", type=positive_int)
    measure.add_argument("--cpu-quota", type=positive)
    return cli


@contextmanager
def server(args, command, directory, peer_settings):
    budget = Budget(args.cgroup_parent, args.memory_mib, args.cpu_quota)
    failure = None
    try:
        network_settings = peer_settings if Path(command[0]).name == "network_bench" else None
        with Process(command, directory, peer_settings=network_settings,
                     cpus=(args.server_cpu,) if args.server_cpu is not None else None,
                     budget=budget) as process:
            yield process, budget
    except BaseException as error:
        failure = error
        raise
    finally:
        try:
            budget.close()
            require_no_oom(budget.final)
        except BaseException as cleanup_error:
            if failure is None:
                raise
            failure.add_note(f"cannot close benchmark budget: {cleanup_error}")


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def orm_measurements(text, expected_name, expected_samples, expected_checksum, elapsed_seconds):
    samples = []
    for line in text.splitlines():
        if not line.startswith("SAMPLE|"):
            continue
        fields = line.split("|")
        if len(fields) != 5:
            raise ValueError(f"invalid ORM sample: {line}")
        _, name, operations, nanos, checksum = fields
        sample = {"name": name, "operations": int(operations), "nanos": int(nanos),
                  "checksum": int(checksum)}
        if (name != expected_name or sample["operations"] <= 0 or sample["nanos"] <= 0
                or sample["checksum"] != expected_checksum
                or len(samples) >= expected_samples):
            raise ValueError(f"invalid ORM sample: {line}")
        samples.append(sample)
    if len(samples) != expected_samples:
        raise ValueError(
            f"ORM {expected_name} returned {len(samples)} samples, expected {expected_samples}"
        )
    latencies = [sample["nanos"] / 1_000_000 for sample in samples]
    operations = sum(sample["operations"] for sample in samples)
    if elapsed_seconds <= 0:
        raise ValueError("ORM measurement duration must be positive")
    return {
        "samples": len(samples),
        "operations": operations,
        "elapsed_seconds": elapsed_seconds,
        "throughput_operations_per_second": operations / elapsed_seconds,
        "latency_ms": {
            "p50": percentile(latencies, 0.50),
            "p95": percentile(latencies, 0.95),
            "p99": percentile(latencies, 0.99),
        },
        "raw": samples,
    }


def expected_orm_checksum(entry, settings):
    rows = settings["orm_rows"]
    return {
        "crud": 2,
        "list": rows + min(32, rows),
        "cursor": min(32, rows),
        "stream": rows,
        "pool": rows,
    }[entry]


def async_measurements(text, settings):
    samples = []
    for line in text.splitlines():
        if not line.startswith("SAMPLE|"):
            continue
        _, name, count, nanos, checksum = line.split("|")
        count, nanos, checksum = int(count), int(nanos), int(checksum)
        expected_count = settings["pending"] if name == "cancel" else settings["iterations"]
        expected_checksum = count if name == "cancel" else count * ord("x")
        if name not in ("tasks", "channels", "cancel") or count != expected_count or checksum != expected_checksum or nanos <= 0:
            raise ValueError(f"invalid benchmark result: {line}")
        samples.append({"name": name, "operations": count, "nanos": nanos, "checksum": checksum})
    if not samples:
        raise ValueError("missing timing samples")
    expected_samples = 1 if samples[0]["name"] == "cancel" else 10
    if len(samples) != expected_samples or len({sample["name"] for sample in samples}) != 1:
        raise ValueError("incomplete or mixed timing samples")
    measured = samples if expected_samples == 1 else samples[1:]
    per_operation = [sample["nanos"] / sample["operations"] for sample in measured]
    return {"raw": samples, "warmup_samples": expected_samples != 1,
            "median_ns_per_operation": statistics.median(per_operation),
            "p95_ns_per_operation_sample": percentile(per_operation, 0.95)}


def write_case(output, case):
    with (output / "cases.jsonl").open("a") as record:
        record.write(json.dumps(case, ensure_ascii=False) + "\n")
    return case


def validate_load(result):
    counters = ("scheduled", "sent", "completed", "succeeded", "errors", "dropped",
                "dropped_capacity", "dispatch_expired", "latency_samples", "request_latency_samples", "dispatch_lag_samples")
    if any(type(result[key]) is not int or result[key] < 0 for key in counters):
        raise ValueError("load counts must be nonnegative integers")
    if (result["scheduled"] != result["sent"] + result["dropped"]
            or result["sent"] != result["completed"]
            or result["completed"] != result["succeeded"] + result["errors"]
            or result["dropped"] != result["dropped_capacity"] + result["dispatch_expired"]
            or result["latency_samples"] != result["succeeded"]
            or result["request_latency_samples"] != result["succeeded"]
            or result["dispatch_lag_samples"] > result["completed"]):
        raise ValueError("load counts do not account for all scheduled requests")
    for metric, count in (("latency_ms", "latency_samples"), ("request_latency_ms", "request_latency_samples"), ("dispatch_lag_ms", "dispatch_lag_samples")):
        values = result[metric]
        if result[count] == 0:
            if values is not None:
                raise ValueError("empty histogram must be null")
        elif (values is None or values.get("sample_count") != result[count]
              or not 0 <= values["p50"] <= values["p95"] <= values["p99"]):
            raise ValueError("invalid latency percentiles")
    if result["elapsed_seconds"] <= 0 or not math.isfinite(result["actual_success_qps"]):
        raise ValueError("invalid load duration/throughput")
    if result["http_version"] not in ("h1", "h2"):
        raise ValueError("invalid HTTP version in load result")
    for name in ("physical_connections", "streams_per_connection", "request_slots"):
        if type(result[name]) is not int or result[name] <= 0:
            raise ValueError("invalid connection shape in load result")
    if result["request_slots"] != result["physical_connections"] * result["streams_per_connection"]:
        raise ValueError("load result connection shape is inconsistent")
    if type(result["successful_handshakes"]) is not int or result["successful_handshakes"] < 0:
        raise ValueError("invalid handshake count in load result")
    if result["http_version"] == "h2" and result["successful_handshakes"] != result["physical_connections"]:
        raise ValueError("HTTP/2 load did not establish its configured physical connections")


def require_no_request_errors(result, stage):
    if result["errors"]:
        raise RuntimeError(f"{stage} completed with {result['errors']} request errors")


def require_successful_requests(result, stage):
    if result["succeeded"] == 0:
        raise RuntimeError(f"{stage} completed without a successful request")


def require_no_oom(budget):
    if budget is None:
        return
    events = budget["events"]
    if any(events[name] for name in ("oom", "oom_kill", "oom_group_kill")):
        raise RuntimeError(f"benchmark cgroup recorded OOM events: {events}")


def memory_cases(args, settings, peer_settings):
    cases = []
    for repeat in range(args.repeats):
        for entry in ("sync_idle", "async_idle", "parked", "http", "https"):
            name = f"memory-{entry}-{repeat}"
            print(name, flush=True)
            binary = "cancel" if entry == "parked" else entry
            command = fixture_command(args.artifacts / binary, binary)
            with server(args, command, args.output / name, peer_settings) as (process, budget):
                ready = process.ready()
                # 采样窗口从 READY 后开始，避免把启动页错误当作空闲基线。
                first = len(process.samples)
                observe([process], args.idle_seconds, args.interval)
                if process.child.poll() is not None:
                    raise RuntimeError(f"idle fixture ended before sampling completed: {entry}")
                case = {"suite": "memory", "entry": entry, "repeat": repeat,
                        "ready_seconds": process.ready_seconds, "ready_value": ready,
                        "resources": summarize(process.samples[first:]), "raw": process.samples,
                        "budget_before_stop": budget.sample()}
            case["budget_after_stop"] = budget.final
            cases.append(write_case(args.output, case))
    return cases


def async_cases(args, settings, peer_settings):
    cases = []
    workloads = [("dever", "root", "tasks", fixture_command(args.artifacts / "tasks", "tasks")),
                 ("dever", "worker", "tasks", fixture_command(args.artifacts / "tasks_on_worker", "tasks_on_worker"))]
    for mode in ("root", "worker"):
        workloads.append(("runtime", mode, "tasks", [args.artifacts / "async_bench", mode,
                          settings["workers"], settings["task_capacity"], settings["iterations"]]))
    workloads.extend(("dever", "root", entry, fixture_command(args.artifacts / entry, entry))
                     for entry in ("channels", "cancel"))
    for repeat in range(args.repeats):
        for implementation, mode, entry, command in workloads[::1 if repeat % 2 == 0 else -1]:
            name = f"async-{implementation}-{mode}-{entry}-{repeat}"
            print(name, flush=True)
            with server(args, command, args.output / name, peer_settings) as (process, budget):
                process.ready()
                deadline = time.monotonic() + 60
                while process.child.poll() is None:
                    if time.monotonic() >= deadline:
                        raise TimeoutError(f"async case exceeded 60 seconds: {entry}")
                    process.sample()
                    time.sleep(args.interval)
                process.check()
                case = {"suite": "async", "entry": entry, "repeat": repeat,
                        "implementation": implementation, "mode": mode,
                        "timing": async_measurements(process.text(), settings),
                        "resources": summarize(process.samples), "raw": process.samples,
                        "budget_before_stop": budget.sample()}
            case["budget_after_stop"] = budget.final
            cases.append(write_case(args.output, case))
    return cases


def load(args, process, directory, http_version, url, path, rate, duration,
         physical_connections, streams_per_connection, reuse, timeout_ms, *, orm_count=None):
    command = [args.artifacts / "network_bench", "load", http_version, url, path, rate, duration,
               physical_connections, streams_per_connection, timeout_ms, str(reuse).lower(),
               args.artifacts / "tls/root.pem"]
    client_settings = {"workers": args.client_workers,
                       "http2_stream_window_bytes": args.http2_stream_window_bytes,
                       "http2_connection_window_bytes": args.http2_connection_window_bytes}
    if orm_count is not None:
        client_settings["orm_count"] = orm_count
    with Process(command, directory, cpus=args.client_cpus, peer_settings=client_settings) as client:
        deadline = time.monotonic() + duration + timeout_ms / 1000 + 10
        while client.child.poll() is None:
            if time.monotonic() >= deadline:
                raise TimeoutError(f"load client exceeded deadline: {directory}")
            process.sample()
            process.check()
            if process.child.poll() is not None:
                raise RuntimeError("server exited during load")
            client.sample()
            time.sleep(args.interval)
        client.check()
        result = json.loads(client.text())
        validate_load(result)
        expected_shape = (http_version, physical_connections, streams_per_connection)
        actual_shape = (result["http_version"], result["physical_connections"],
                        result["streams_per_connection"])
        if actual_shape != expected_shape:
            raise ValueError(f"load result shape {actual_shape} does not match {expected_shape}")
        result["client_resources"] = summarize(client.samples)
        result["client_raw"] = client.samples
        return result


def http_cases(args, settings, peer_settings):
    cases = []
    modes = [True, False] if args.connection_mode == "both" else [args.connection_mode == "reuse"]
    for repeat in range(args.repeats):
        # 各轮倒转顺序，降低热机/后台负载变化对某个实现的固定偏置。
        implementations = implementation_order(args.implementations, repeat)
        workloads = product(("http", "https"), args.paths.split(","), args.rates.split(","), modes, implementations)
        for protocol, path, rate, reuse, implementation in workloads:
            name = f"{implementation}-{protocol}-{path[1:]}-{rate}-{reuse}-{repeat}"
            print(name, flush=True)
            command = fixture_command(args.artifacts / protocol, protocol) if implementation == "dever" else [args.artifacts / "network_bench", f"{implementation}-{protocol}"]
            with server(args, command, args.output / name, peer_settings) as (process, budget):
                port = process.ready()
                url = f"{protocol}://localhost:{port}"
                warmup = load(args, process, process.directory / "warmup", "h1", url, path,
                              rate, args.warmup, args.concurrency, 1, reuse, settings["timeout_ms"])
                require_no_request_errors(warmup, "HTTP warmup")
                require_successful_requests(warmup, "HTTP warmup")
                first = len(process.samples)
                result = load(args, process, process.directory / "load", "h1", url, path,
                              rate, args.duration, args.concurrency, 1, reuse, settings["timeout_ms"])
                require_no_request_errors(result, "HTTP measurement")
                require_successful_requests(result, "HTTP measurement")
                case = {"suite": "http", "implementation": implementation,
                        "protocol": protocol, "path": path, "rate": float(rate),
                        "reuse": reuse, "repeat": repeat, "warmup": warmup, "load": result,
                        "resources": summarize(process.samples[first:]),
                        "raw": process.samples, "budget_before_stop": budget.sample()}
            case["budget_after_stop"] = budget.final
            cases.append(write_case(args.output, case))
    return cases


def http2_server_command(args, implementation, transport):
    if implementation == "dever":
        entry = "http2" if transport == "http" else "https2"
        return fixture_command(args.artifacts / entry, entry)
    mode = "http2" if transport == "http" else "https2"
    return [args.artifacts / "network_bench", f"{implementation}-{mode}"]


def observe_disconnected(args, process, baseline_fds):
    first = len(process.samples)
    observe([process], HTTP2_DISCONNECT_SECONDS, args.interval)
    samples = process.samples[first:]
    resources = summarize(samples)
    last = samples[-1]
    fd_delta = last["fds"] - baseline_fds
    if fd_delta != 0:
        raise RuntimeError(f"HTTP/2 connections were not released: FD delta {fd_delta}")
    return {"last": last, "resources": resources, "fd_delta": fd_delta}


def http2_cases(args, settings, peer_settings):
    cases = []
    streams = settings["http2_streams"]
    for repeat in range(args.repeats):
        implementations = implementation_order(args.implementations, repeat)
        workloads = product(("http", "https"), args.paths.split(","), args.rates.split(","),
                            args.http2_connections, implementations)
        for transport, path, rate, connections, implementation in workloads:
            name = (f"{implementation}-h2-{transport}-{path[1:]}-{rate}-"
                    f"{connections}x{streams}-{repeat}")
            print(name, flush=True)
            command = http2_server_command(args, implementation, transport)
            with server(args, command, args.output / name, peer_settings) as (process, budget):
                port = process.ready()
                url = f"{transport}://localhost:{port}"
                baseline_first = len(process.samples)
                observe([process], HTTP2_DISCONNECT_SECONDS, args.interval)
                baseline_samples = process.samples[baseline_first:]
                baseline = summarize(baseline_samples)
                baseline_fds = baseline_samples[-1]["fds"]
                warmup = load(args, process, process.directory / "warmup", "h2", url, path,
                              rate, args.warmup, connections, streams, True, settings["timeout_ms"])
                warmup_disconnected = observe_disconnected(args, process, baseline_fds)
                first = len(process.samples)
                result = load(args, process, process.directory / "load", "h2", url, path,
                              rate, args.duration, connections, streams, True, settings["timeout_ms"])
                load_end = len(process.samples)
                disconnected = observe_disconnected(args, process, baseline_fds)
                case = {"suite": "http2", "implementation": implementation,
                        "http_version": "h2", "transport": transport, "path": path,
                        "body_bytes": HTTP_BODY_BYTES[path],
                        "rate": float(rate), "physical_connections": connections,
                        "streams_per_connection": streams,
                        "request_slots": connections * streams,
                        "stream_window_bytes": settings["http2_stream_window_bytes"],
                        "connection_window_bytes": settings["http2_connection_window_bytes"],
                        "frame_bytes": 16_384, "repeat": repeat,
                        "baseline": baseline, "warmup": warmup,
                        "warmup_disconnected": warmup_disconnected,
                        "load": result, "resources": summarize(process.samples[first:load_end]),
                        "disconnected": disconnected, "raw": process.samples,
                        "budget_before_stop": budget.sample()}
            case["budget_after_stop"] = budget.final
            cases.append(write_case(args.output, case))
            require_no_request_errors(warmup, "HTTP/2 warmup")
            require_no_request_errors(result, "HTTP/2 load")
            require_no_oom(case["budget_after_stop"])
    return cases


def http2_recovery_cases(args, settings, peer_settings):
    cases = []
    streams = settings["http2_streams"]
    connections = args.recovery_connections
    for repeat in range(args.repeats):
        for transport, implementation in product(("http", "https"),
                                                  implementation_order(args.implementations, repeat)):
            name = f"{implementation}-h2-recovery-{transport}-{connections}x{streams}-{repeat}"
            print(name, flush=True)
            command = http2_server_command(args, implementation, transport)
            with server(args, command, args.output / name, peer_settings) as (process, budget):
                port = process.ready()
                url = f"{transport}://localhost:{port}"
                baseline_first = len(process.samples)
                observe([process], HTTP2_DISCONNECT_SECONDS, args.interval)
                baseline_samples = process.samples[baseline_first:]
                baseline = summarize(baseline_samples)
                baseline_fds = baseline_samples[-1]["fds"]
                first = len(process.samples)
                cycles = []
                for cycle in range(args.cycles):
                    result = load(args, process, process.directory / f"cycle-{cycle}", "h2", url,
                                  args.recovery_path, args.recovery_rate, args.duration,
                                  connections, streams, True, settings["timeout_ms"])
                    if result["succeeded"] == 0:
                        raise RuntimeError(f"HTTP/2 recovery cycle {cycle} did not complete cleanly")
                    require_no_request_errors(result, f"HTTP/2 recovery cycle {cycle}")
                    disconnected = observe_disconnected(args, process, baseline_fds)
                    cycles.append({"cycle": cycle, "load": result, "disconnected": disconnected})
                case = {"suite": "http2-recovery", "implementation": implementation,
                        "http_version": "h2", "transport": transport,
                        "path": args.recovery_path, "rate": args.recovery_rate,
                        "physical_connections": connections,
                        "streams_per_connection": streams,
                        "request_slots": connections * streams,
                        "stream_window_bytes": settings["http2_stream_window_bytes"],
                        "connection_window_bytes": settings["http2_connection_window_bytes"],
                        "frame_bytes": 16_384, "cycles": cycles,
                        "repeat": repeat, "baseline": baseline,
                        "resources": summarize(process.samples[first:]),
                        "raw": process.samples, "budget_before_stop": budget.sample()}
            case["budget_after_stop"] = budget.final
            cases.append(write_case(args.output, case))
            require_no_oom(case["budget_after_stop"])
    return cases


def http2_summary(cases):
    groups = {}
    for case in cases:
        if case["suite"] != "http2":
            continue
        key = (case["implementation"], case["transport"], case["path"], case["rate"],
               case["physical_connections"], case["streams_per_connection"])
        groups.setdefault(key, []).append(case)
    summary = []
    for key, repeated in sorted(groups.items()):
        implementation, transport, path, rate, connections, streams = key
        p99_values = [case["load"]["request_latency_ms"]["p99"] for case in repeated
                      if case["load"]["request_latency_ms"] is not None]
        summary.append({
            "implementation": implementation,
            "transport": transport,
            "path": path,
            "rate": rate,
            "physical_connections": connections,
            "streams_per_connection": streams,
            "request_slots": connections * streams,
            "repeats": len(repeated),
            "actual_success_qps_median": statistics.median(
                case["load"]["actual_success_qps"] for case in repeated),
            "request_latency_p99_ms_median": statistics.median(p99_values) if p99_values else None,
            "dropped_median": statistics.median(case["load"]["dropped"] for case in repeated),
            "server_rss_bytes_median": statistics.median(
                case["resources"]["rss_bytes_median"] for case in repeated),
            "server_pss_bytes_median": statistics.median(
                case["resources"]["pss_bytes_median"] for case in repeated),
            "server_rss_high_water_bytes_median": statistics.median(
                case["resources"]["rss_high_water_bytes"] for case in repeated),
        })
    return summary


def stage_case_binary(args, case_name, binary_name):
    bundle = args.output / "bundles" / case_name
    bundle.mkdir(parents=True, exist_ok=False)
    executable = bundle / "app"
    shutil.copy2(args.artifacts / binary_name, executable)
    return executable


def stage_live_bundle(args, settings, case_name, protocol):
    executable = stage_case_binary(args, case_name, protocol)
    config = executable.parent / "config"
    config.mkdir()
    write_private_json(config / "setting.json", native_runtime_setting(settings))
    return executable


def live_cases(args, settings, peer_settings):
    cases = []
    for repeat, protocol, connections in product(range(args.repeats), ("tcp", "ws", "sse"),
                                                  map(int, args.connection_tiers.split(","))):
        name = f"live-{protocol}-{connections}-{repeat}"
        print(name, flush=True)
        executable = stage_live_bundle(args, settings, name, protocol)
        with server(args, fixture_command(executable, protocol), args.output / name, peer_settings) as (process, budget):
            port = process.ready()
            first = len(process.samples)
            observe([process], 0.3, args.interval)
            baseline = summarize(process.samples[first:])
            measured = load_live(args, process, port, protocol, connections, settings["timeout_ms"])
            disconnected = [phase["last"] for phase in measured["phases"] if phase["phase"] == "disconnected"]
            fd_deltas = [sample["fds"] - baseline["fds_median"] for sample in disconnected]
            case = {"suite": "live", "protocol": protocol, "connections": connections,
                    "repeat": repeat, "baseline": baseline, **measured,
                    "fd_delta_after_cycles": fd_deltas,
                    "rss_delta_after_cycles": [sample["rss_bytes"] - baseline["rss_bytes_median"] for sample in disconnected],
                    "budget_before_stop": budget.sample()}
        case["budget_after_stop"] = budget.final
        cases.append(write_case(args.output, case))
        if any(delta != 0 for delta in fd_deltas):
            raise RuntimeError(f"live {protocol} did not release file descriptors: {fd_deltas}")
    return cases


def report_arguments(args):
    values = {key: str(value) if isinstance(value, Path) else value
              for key, value in vars(args).items() if not key.startswith("_")}
    return values


def required_binaries(args):
    suites = {
        "memory": {"sync_idle", "async_idle", "cancel", "http", "https"},
        "async": {"tasks", "tasks_on_worker", "channels", "cancel", "async_bench"},
        "http": {"http", "https", "network_bench"},
        "live": {"tcp", "ws", "sse", "live_bench"},
        "http2": {"http2", "https2", "network_bench"},
        "http2-recovery": {"http2", "https2", "network_bench"},
        "orm": {"network_bench", *(f"sqlite-{entry}" for entry in ORM_ENTRIES)},
        "cms": {"cms"},
    }
    if args._postgres_setting is not None:
        suites["orm"].update(f"postgres-{entry}" for entry in ORM_ENTRIES)
    if "md" in args._cms_sources:
        suites["cms"].add("cms-md")
    selected = suites if args.suite == "all" else {args.suite: suites[args.suite]}
    return set().union(*selected.values())


def verify_artifacts(directory, manifest, required):
    if manifest.get("peer_configuration") != PEER_CONFIGURATION:
        raise ValueError("benchmark artifacts use an obsolete configuration contract; rebuild them")
    if manifest.get("application_configuration") != APPLICATION_CONFIGURATION:
        raise ValueError("benchmark artifacts use an obsolete application contract; rebuild with the current LLVM compiler")
    binaries = manifest["binaries"]
    missing = sorted(required - binaries.keys())
    if missing:
        raise ValueError(f"manifest is missing required binaries: {', '.join(missing)}")
    for group in (binaries, manifest["files"]):
        for name, recorded in group.items():
            actual = fingerprint(directory / name)
            if actual != {"sha256": recorded["sha256"], "bytes": recorded["bytes"]}:
                raise ValueError(f"artifact changed since build: {name}")


def stage_orm_bundle(args, settings, driver, entry, repeat, *, binary_entry=None,
                     pool_capacity=None):
    case_name = f"orm-{driver}-{entry}-{repeat}"
    binary_name = f"{driver}-{binary_entry or entry}"
    executable = stage_case_binary(args, case_name, binary_name)
    bundle = executable.parent
    if driver == "postgres":
        setting = database_setting(
            driver,
            settings,
            args._postgres_setting,
            postgres_database_case(args.output, case_name),
        )
    else:
        setting = database_setting(driver, settings)
    if pool_capacity is not None:
        setting["database"]["default"]["max_connections"] = pool_capacity
    if entry in ("http", "cancel"):
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        setting["http"] = {"host": "127.0.0.1", "port": port}
    config = bundle / "config"
    config.mkdir()
    setting_path = config / "setting.json"
    write_private_json(setting_path, setting)
    return case_name, executable, args.output / case_name, binary_name


def orm_observability(settings, driver, pool_capacity=None):
    capacity = pool_capacity or settings[f"orm_{driver}_connections"]
    return {
        "configured_pool_capacity": capacity,
        "pool_wait": {"status": "unavailable",
                      "reason": "runtime does not expose pool acquisition timing"},
        "sql_count": {"status": "unavailable",
                      "reason": "runtime does not expose executed statement counters"},
        "database_connections": {
            "status": "unavailable",
            "reason": "process FD count is reported, but database connection identity is not observable",
        },
    }


def finite_orm_case(args, settings, peer_settings, driver, entry, repeat):
    name, executable, directory, binary_name = stage_orm_bundle(
        args, settings, driver, entry, repeat
    )
    print(name, flush=True)
    if entry in ("stream", "pool"):
        seed_orm_bundle(executable)
    with server(args, application_command(executable), directory, peer_settings) as (process, budget):
        ready = process.ready()
        first = max(0, len(process.samples) - 1)
        measured_at = time.monotonic()
        deadline = measured_at + max(30, args.duration + args.warmup + 20)
        while process.child.poll() is None:
            if time.monotonic() >= deadline:
                raise TimeoutError(f"ORM case exceeded deadline: {name}")
            process.sample()
            process.check()
            time.sleep(args.interval)
        elapsed = time.monotonic() - measured_at
        process.check()
        expected_samples = 1 if entry == "cancel" else settings["orm_iterations"]
        measured = orm_measurements(
            process.text(), entry, expected_samples,
            expected_orm_checksum(entry, settings), elapsed,
        )
        samples = process.samples[first:]
        if not samples:
            raise ValueError(f"ORM case ended before a resource sample: {name}")
        case = {
            "suite": "orm",
            "driver": driver,
            "entry": entry,
            "repeat": repeat,
            "ready_seconds": process.ready_seconds,
            "ready_value": ready,
            "binary": {"name": binary_name, **fingerprint(executable)},
            "measurement": measured,
            "resources": summarize(samples),
            "raw": process.samples,
            "observability": orm_observability(settings, driver),
            "budget_before_stop": budget.sample(),
        }
    case["budget_after_stop"] = budget.final
    require_no_oom(case["budget_after_stop"])
    return write_case(args.output, case)


def idle_orm_case(args, settings, peer_settings, driver, repeat):
    name, executable, directory, binary_name = stage_orm_bundle(
        args, settings, driver, "idle", repeat
    )
    print(name, flush=True)
    with server(args, application_command(executable), directory, peer_settings) as (process, budget):
        ready = process.ready()
        first = len(process.samples)
        observe([process], args.idle_seconds, args.interval)
        samples = process.samples[first:]
        case = {
            "suite": "orm",
            "driver": driver,
            "entry": "idle",
            "repeat": repeat,
            "ready_seconds": process.ready_seconds,
            "ready_value": ready,
            "binary": {"name": binary_name, **fingerprint(executable)},
            "resources": summarize(samples),
            "raw": process.samples,
            "observability": orm_observability(settings, driver),
            "budget_before_stop": budget.sample(),
        }
    case["budget_after_stop"] = budget.final
    require_no_oom(case["budget_after_stop"])
    return write_case(args.output, case)


def http_orm_case(args, settings, peer_settings, driver, repeat):
    name, executable, directory, binary_name = stage_orm_bundle(
        args, settings, driver, "http", repeat
    )
    print(name, flush=True)
    rate = positive_int(args.rates.split(",")[0])
    seed_orm_bundle(executable)
    expected_count = settings["orm_rows"]
    port = orm_http_port(executable)
    with server(args, [executable], directory, peer_settings) as (process, budget):
        wait_orm_ready(process, port, expected_count)
        url = f"http://127.0.0.1:{port}"
        warmup = load(args, process, process.directory / "warmup", "h1", url, ORM_COUNT_PATH,
                      rate, args.warmup, args.concurrency, 1, True, settings["timeout_ms"],
                      orm_count=expected_count)
        first = len(process.samples)
        result = load(args, process, process.directory / "load", "h1", url, ORM_COUNT_PATH,
                      rate, args.duration, args.concurrency, 1, True, settings["timeout_ms"],
                      orm_count=expected_count)
        case = {
            "suite": "orm",
            "driver": driver,
            "entry": "http",
            "repeat": repeat,
            "rate": rate,
            "warmup": warmup,
            "load": result,
            "binary": {"name": binary_name, **fingerprint(executable)},
            "resources": summarize(process.samples[first:]),
            "raw": process.samples,
            "observability": orm_observability(settings, driver),
            "budget_before_stop": budget.sample(),
        }
    case["budget_after_stop"] = budget.final
    require_no_request_errors(warmup, "ORM HTTP warmup")
    require_no_request_errors(result, "ORM HTTP load")
    require_successful_requests(warmup, "ORM HTTP warmup")
    require_successful_requests(result, "ORM HTTP load")
    require_no_oom(case["budget_after_stop"])
    return write_case(args.output, case)


def orm_http_port(executable):
    return json.loads((executable.parent / "config/setting.json").read_text())["http"]["port"]


def seed_orm_bundle(executable):
    result = subprocess.run([executable, "benchmark.item.seed", "{}"], cwd=executable.parent,
                            capture_output=True, text=True, timeout=30)
    if result.returncode != 0:
        raise RuntimeError(f"ORM seed failed: {result.stderr or result.stdout}")


def http_data(port, path, timeout):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    try:
        connection.request("GET", path, headers={"Connection": "close"})
        response = connection.getresponse()
        body = response.read().decode("utf-8")
        if response.status != 200:
            raise RuntimeError(f"ORM probe {path} returned HTTP {response.status}")
        if response.getheader("Content-Type") != "application/json; charset=utf-8":
            raise ValueError(f"ORM probe {path} returned a non-JSON response")
        envelope = json.loads(body)
        if (type(envelope) is not dict or set(envelope) != {"code", "message", "data"}
                or type(envelope["code"]) is not int or envelope["code"] != 0
                or envelope["message"] != "ok"):
            raise ValueError(f"ORM probe {path} returned an invalid API envelope")
        return envelope["data"]
    finally:
        connection.close()


def orm_count(port, timeout):
    count = http_data(port, ORM_COUNT_PATH, timeout)
    if type(count) is not int or count < 0:
        raise ValueError("ORM count API returned a nonnegative-integer violation")
    return count


def wait_orm_ready(process, port, expected_count):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        process.sample()
        process.check()
        if process.child.poll() is not None:
            raise RuntimeError(f"ORM HTTP service exited before readiness: {process.directory}")
        try:
            count = orm_count(port, 0.2)
        except (ConnectionError, TimeoutError, socket.timeout):
            time.sleep(0.01)
            continue
        if count != expected_count:
            raise RuntimeError(f"ORM HTTP service started with {count} rows, expected {expected_count}")
        process.ready_seconds = time.monotonic() - process.started
        return
    raise TimeoutError(f"ORM HTTP readiness timeout: {process.directory}")


def transaction_cancel_measurement(remaining, elapsed_seconds):
    if remaining != 0:
        raise RuntimeError(f"cancelled transaction committed {remaining} row(s)")
    if elapsed_seconds <= 0:
        raise ValueError("transaction cancellation duration must be positive")
    latency = elapsed_seconds * 1000
    return {
        "samples": 1,
        "operations": 1,
        "elapsed_seconds": elapsed_seconds,
        "throughput_operations_per_second": 1 / elapsed_seconds,
        "latency_ms": {"p50": latency, "p95": latency, "p99": latency},
        "raw": [{"name": "cancel", "operations": 1,
                 "nanos": round(elapsed_seconds * 1_000_000_000),
                 "checksum": remaining}],
    }


def cancel_orm_case(args, settings, peer_settings, driver, repeat):
    name, executable, directory, binary_name = stage_orm_bundle(
        args, settings, driver, "cancel", repeat, binary_entry="http", pool_capacity=1
    )
    print(name, flush=True)
    port = orm_http_port(executable)
    with server(args, [executable], directory, peer_settings) as (process, budget):
        wait_orm_ready(process, port, 0)
        baseline_count = orm_count(port, 0.2)
        if baseline_count != 0:
            raise RuntimeError(f"cancel fixture started with {baseline_count} row(s)")
        first = len(process.samples)
        cancel = socket.create_connection(("127.0.0.1", port), timeout=settings["timeout_ms"] / 1000)
        try:
            cancel.sendall(
                b"POST /benchmark/item/cancel HTTP/1.1\r\nHost: localhost\r\n"
                b"Content-Type: application/json\r\nContent-Length: 2\r\n"
                b"Connection: close\r\n\r\n{}"
            )
            blocked = False
            handler_timeout = settings["timeout_ms"] / 1000
            probe_deadline = time.monotonic() + handler_timeout * 0.5
            probe_attempts = 0
            for probe_attempts in range(1, 65):
                if time.monotonic() >= probe_deadline:
                    break
                process.sample()
                process.check()
                if select.select([cancel], [], [], 0)[0]:
                    raise RuntimeError("transaction cancel request completed before pool saturation")
                try:
                    orm_count(port, 0.05)
                except (TimeoutError, socket.timeout):
                    blocked = True
                    break
            if not blocked:
                raise TimeoutError("transaction did not occupy the only pool connection")
            cancelled_at = time.monotonic()
        finally:
            try:
                cancel.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            cancel.close()

        recovery_deadline = cancelled_at + handler_timeout * 0.75
        remaining = None
        recovery_attempts = 0
        for recovery_attempts in range(1, 51):
            if time.monotonic() >= recovery_deadline:
                break
            process.sample()
            process.check()
            try:
                remaining = orm_count(port, 0.2)
                break
            except (TimeoutError, socket.timeout):
                continue
        if remaining is None:
            raise TimeoutError("database connection did not return after request cancellation")
        elapsed = time.monotonic() - cancelled_at
        samples = process.samples[first:]
        case = {
            "suite": "orm",
            "driver": driver,
            "entry": "cancel",
            "repeat": repeat,
            "binary": {"name": binary_name, **fingerprint(executable)},
            "measurement": transaction_cancel_measurement(remaining, elapsed),
            "resources": summarize(samples),
            "raw": process.samples,
            "observability": orm_observability(settings, driver, pool_capacity=1),
            "baseline_count": baseline_count,
            "pool_saturation_observed": blocked,
            "client_disconnected_before_completion": True,
            "connection_recovered_before_handler_timeout": True,
            "probe_attempts": probe_attempts,
            "recovery_attempts": recovery_attempts,
            "budget_before_stop": budget.sample(),
        }
    case["budget_after_stop"] = budget.final
    require_no_oom(case["budget_after_stop"])
    return write_case(args.output, case)


def orm_cases(args, settings, peer_settings):
    drivers = ["sqlite"]
    if args._postgres_setting is not None:
        drivers.append("postgres")
    cases = []
    for repeat in range(args.repeats):
        for driver in drivers:
            cases.append(idle_orm_case(args, settings, peer_settings, driver, repeat))
            for entry in ("crud", "list", "cursor", "stream", "pool"):
                cases.append(finite_orm_case(args, settings, peer_settings, driver, entry, repeat))
            cases.append(cancel_orm_case(args, settings, peer_settings, driver, repeat))
            cases.append(http_orm_case(args, settings, peer_settings, driver, repeat))
    return cases


def orm_summary(cases):
    groups = {}
    for case in cases:
        if case["suite"] != "orm":
            continue
        groups.setdefault((case["driver"], case["entry"]), []).append(case)
    summary = []
    for (driver, entry), repeated in sorted(groups.items()):
        item = {
            "driver": driver,
            "entry": entry,
            "repeats": len(repeated),
            "binary_bytes": repeated[0]["binary"]["bytes"],
            "rss_bytes_median": statistics.median(
                case["resources"]["rss_bytes_median"] for case in repeated),
            "pss_bytes_median": statistics.median(
                case["resources"]["pss_bytes_median"] for case in repeated),
            "rss_high_water_bytes_median": statistics.median(
                case["resources"]["rss_high_water_bytes"] for case in repeated),
        }
        measurements = [case["measurement"] for case in repeated if "measurement" in case]
        loads = [case["load"] for case in repeated if "load" in case]
        if measurements:
            item.update({
                "throughput_operations_per_second_median": statistics.median(
                    value["throughput_operations_per_second"] for value in measurements),
                "latency_p50_ms_median": statistics.median(
                    value["latency_ms"]["p50"] for value in measurements),
                "latency_p95_ms_median": statistics.median(
                    value["latency_ms"]["p95"] for value in measurements),
                "latency_p99_ms_median": statistics.median(
                    value["latency_ms"]["p99"] for value in measurements),
            })
        elif loads:
            item.update({
                "throughput_operations_per_second_median": statistics.median(
                    value["actual_success_qps"] for value in loads),
                "latency_p50_ms_median": statistics.median(
                    value["request_latency_ms"]["p50"] for value in loads),
                "latency_p95_ms_median": statistics.median(
                    value["request_latency_ms"]["p95"] for value in loads),
                "latency_p99_ms_median": statistics.median(
                    value["request_latency_ms"]["p99"] for value in loads),
            })
        summary.append(item)
    return summary


def cms_cases(args, _settings, _peer_settings):
    from cms import run_case

    cases = []
    for repeat in range(args.repeats):
        for source in args._cms_sources:
            name = f"cms-{source}-{repeat}"
            binary_name = "cms" if source == "dever" else "cms-md"
            executable = args.artifacts / binary_name
            identity = {
                "executable": str(executable),
                "config": str(args.artifacts / "source" / f"cms-{source}" / "config/setting.json"),
                "fingerprint": fingerprint(executable),
            }
            print(name, flush=True)
            result = run_case(
                identity, args.output / name, articles=16,
                cgroup_parent=args.cgroup_parent, memory_mib=args.memory_mib,
                idle_seconds=args.idle_seconds, interval=args.interval,
                server_cpu=args.server_cpu, cpu_quota=args.cpu_quota,
            )
            require_no_oom(result["cgroup"])
            case = {"suite": "cms", "source": source, "repeat": repeat,
                    "workload_mode": "sequential_publish",
                    "binary": {"name": binary_name, **identity["fingerprint"]}, **result}
            cases.append(write_case(args.output, case))
    if len({json.dumps(case["contract"], sort_keys=True) for case in cases}) != 1:
        raise RuntimeError("Dever and Markdown CMS changed the publish contract")
    return cases


def cms_summary(cases):
    repeated = [case for case in cases if case["suite"] == "cms"]
    if not repeated:
        return None
    return {source: cms_source_summary([case for case in repeated
                                        if case["source"] == source])
            for source in sorted({case["source"] for case in repeated})}


def cms_source_summary(repeated):
    peaks = [case["cgroup"]["peak_bytes"] for case in repeated
             if case["cgroup"] is not None]
    return {
        "repeats": len(repeated),
        "workload_mode": "sequential_publish",
        "binary_bytes": repeated[0]["binary"]["bytes"],
        "articles": repeated[0]["contract"]["articles"],
        "ready_seconds_median": statistics.median(
            case["ready_seconds"] for case in repeated),
        "workload_seconds_median": statistics.median(
            case["workload_seconds"] for case in repeated),
        "published_articles_per_second_median": statistics.median(
            case["published_articles_per_second"] for case in repeated),
        "rss_bytes_median": statistics.median(
            case["resources"]["rss_bytes_median"] for case in repeated),
        "pss_bytes_median": statistics.median(
            case["resources"]["pss_bytes_median"] for case in repeated),
        "rss_high_water_bytes_median": statistics.median(
            case["resources"]["rss_high_water_bytes"] for case in repeated),
        "cgroup_peak_bytes_median": statistics.median(peaks) if peaks else None,
    }


def measure(args):
    args.artifacts = args.artifacts.resolve()
    args.output = args.output.resolve()
    args._postgres_setting = (
        load_postgres_setting() if args.suite in ("all", "orm") else None
    )
    manifest = json.loads((args.artifacts / "manifest.json").read_text())
    args._cms_sources = ("dever", "md") if "cms-md" in manifest["binaries"] else ("dever",)
    verify_artifacts(args.artifacts, manifest, required_binaries(args))
    settings = manifest["settings"]
    args.http2_stream_window_bytes = settings["http2_stream_window_bytes"]
    args.http2_connection_window_bytes = settings["http2_connection_window_bytes"]
    args.implementations = parse_implementations(args.implementations)
    args.http2_connections = csv_positive_ints(args.http2_connections, "http2-connections")
    if args.suite in ("all", "http") and args.concurrency > settings["connections"]:
        raise ValueError("client concurrency must not exceed compiled server connections")
    if args.idle_seconds >= settings["idle_sample_ms"] / 1000:
        raise ValueError("idle window must be shorter than the parked fixture's 3-second hold")
    if any(path not in ("/plain", "/json", "/bytes") for path in args.paths.split(",")):
        raise ValueError("paths must be /plain, /json or /bytes")
    for rate in args.rates.split(","):
        positive_int(rate)
    if args.duration + args.warmup + 20 >= settings["lifetime_ms"] / 1000:
        raise ValueError("measurement exceeds fixture lifetime")
    if args.suite in ("all", "http2", "http2-recovery"):
        streams = settings["http2_streams"]
        physical_connections = (*args.http2_connections, args.recovery_connections)
        if max(physical_connections) > settings["connections"]:
            raise ValueError("HTTP/2 physical connections exceed compiled server capacity")
        if max(physical_connections) * streams > 65_536:
            raise ValueError("HTTP/2 request slots exceed the language limit")
        if max(physical_connections) * (streams + 1) > settings["task_capacity"]:
            raise ValueError("HTTP/2 topology exceeds compiled task capacity")
        if (args.suite in ("all", "http2-recovery")
                and args.cycles * (args.duration + HTTP2_DISCONNECT_SECONDS) + 20
                >= settings["lifetime_ms"] / 1000):
            raise ValueError("HTTP/2 recovery cycles exceed fixture lifetime")
    if args.suite in ("all", "live"):
        tiers = [positive_int(value) for value in args.connection_tiers.split(",")]
        if max(tiers) > min(settings["connections"], 4096) or args.cycles > 100:
            raise ValueError("long connection workload exceeds configured capacity")
        if not 0.3 <= args.duration <= 60 or args.interval > 0.1:
            raise ValueError("live duration must be 0.3..60 seconds and interval <= 0.1 seconds")
        if lifecycle_seconds(args.cycles, args.duration, settings["timeout_ms"]) >= settings["lifetime_ms"] / 1000:
            raise ValueError("long connection cycles exceed fixture lifetime")
    if args.suite in ("all", "orm"):
        if settings["orm_concurrency"] > settings["orm_iterations"]:
            raise ValueError("ORM concurrency must not exceed ORM iterations")
    allowed = os.sched_getaffinity(0)
    validate_cpu_placement(args.server_cpu, args.client_cpus, allowed)
    peer_settings = {name: settings[name] for name in (
        "workers", "connections", "task_capacity", "timeout_ms", "lifetime_ms",
        "http2_streams", "http2_stream_window_bytes", "http2_connection_window_bytes")}
    peer_settings.update({"cert_path": str(args.artifacts / "tls/server.pem"),
                          "key_path": str(args.artifacts / "tls/server-key.pem")})
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"status": "running", "manifest": manifest,
              "environment": {"platform": platform.platform(), "cpu_count": os.cpu_count(),
                              "client_workers": args.client_workers,
                              "client_cpus": list(args.client_cpus) if args.client_cpus is not None else sorted(allowed),
                              "allowed_cpus": sorted(allowed), "kernel": platform.release(),
                              "cpuinfo": Path("/proc/cpuinfo").read_text(),
                              "parent_cgroup": Path("/proc/self/cgroup").read_text()},
              "arguments": report_arguments(args),
              "cases": []}
    failure = None
    try:
        for suite, execute in (("memory", memory_cases), ("async", async_cases),
                               ("http", http_cases), ("live", live_cases),
                               ("http2", http2_cases),
                               ("http2-recovery", http2_recovery_cases),
                               ("orm", orm_cases), ("cms", cms_cases)):
            if args.suite in ("all", suite):
                report["cases"].extend(execute(args, settings, peer_settings))
        report["status"] = "complete"
    except BaseException as error:
        failure = error
        report["status"] = "failed"
        report["error"] = str(error)
        raise
    finally:
        try:
            # 即使中途失败，也保留已完成 case 与原始日志。
            records = args.output / "cases.jsonl"
            report["cases"] = [json.loads(line) for line in records.read_text().splitlines()] if records.exists() else []
            report["summary"] = {"http2": http2_summary(report["cases"]),
                                 "orm": orm_summary(report["cases"]),
                                 "cms": cms_summary(report["cases"])}
            (args.output / "report.json").write_text(
                json.dumps(report, ensure_ascii=False, indent=2) + "\n"
            )
        except BaseException as cleanup_error:
            if failure is None:
                raise
            failure.add_note(f"cannot persist benchmark report: {cleanup_error}")
    print(f"report: {args.output / 'report.json'}", flush=True)


def main():
    args = parser().parse_args()
    if args.action == "build":
        if (args.workers > 64 or args.connections > 32760 or args.pending > 65520
                or args.iterations > 1_048_576 or args.http2_streams > 65_536
                or args.connections * args.http2_streams > 65_536
                or args.http2_stream_window_bytes > 2_147_483_647
                or not 65_535 <= args.http2_connection_window_bytes <= 2_147_483_647
                or args.orm_iterations > 4096 or args.orm_rows > 10_000
                or args.orm_concurrency > 256 or args.orm_cancel_work > 10_000_000
                or args.orm_sqlite_connections > 64
                or args.orm_concurrency > args.orm_iterations):
            raise ValueError("configuration exceeds bounded fixture limits")
        build(args)
    else:
        measure(args)


if __name__ == "__main__":
    main()

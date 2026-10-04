"""长连接对端的阶段校验与采样；进程预算由统一运行器管理。"""

import json
import time

from process import Process, summarize

PHASES = ("connecting", "connected", "closing", "disconnected")
SETTLE_SECONDS = 0.3
PROCESS_MARGIN_SECONDS = 10


def phase_sequence(cycles):
    return [(cycle, phase) for cycle in range(cycles) for phase in PHASES]


def lifecycle_seconds(cycles, hold_seconds, timeout_ms):
    timeout_seconds = timeout_ms / 1000
    # Each cycle has independent connect, hold-overrun and close deadlines.
    return cycles * (hold_seconds + 3 * timeout_seconds + SETTLE_SECONDS) + PROCESS_MARGIN_SECONDS


def stable_phase(before, after):
    return before[-1] if before and before == after else None


def parse_progress(text, cycles):
    phases = []
    result = None
    # stdout is sampled while its writer is alive; only consume complete lines.
    for line in text.splitlines(keepends=True):
        if not line.endswith("\n"):
            continue
        if result is not None:
            raise ValueError("live benchmark output continued after its final result")
        if line.startswith("PHASE|"):
            _, cycle, phase = line.strip().split("|")
            phases.append((int(cycle), phase))
        elif line.startswith("{"):
            result = json.loads(line)
        else:
            raise ValueError(f"unexpected live benchmark output: {line.strip()}")
    if phases != phase_sequence(cycles)[:len(phases)]:
        raise ValueError("live benchmark phases are missing, duplicated or out of order")
    return phases, result


def validate_result(result, protocol, connections, cycles):
    counts = ("attempted_connections", "connected", "disconnected", "messages", "errors")
    if result is None or any(type(result[key]) is not int or result[key] < 0 for key in counts):
        raise ValueError("missing or invalid live counters")
    expected = connections * cycles
    if (result["protocol"] != protocol or result["connections"] != connections or result["cycles"] != cycles
            or any(result[key] != expected for key in counts[:3])
            or result["messages"] < expected or result["errors"] != 0 or result["elapsed_ms"] <= 0):
        raise ValueError("live benchmark did not validate and close all requested connections")


def load_live(args, process, port, protocol, connections, timeout_ms):
    scheme, path = {"tcp": ("tcp", "/"), "ws": ("ws", "/ws"), "sse": ("http", "/events")}[protocol]
    hold_ms = round(args.duration * 1000)
    command = [args.artifacts / "live_bench", protocol, f"{scheme}://127.0.0.1:{port}{path}",
               connections, args.cycles, hold_ms, timeout_ms]
    phases = {phase: [] for phase in phase_sequence(args.cycles)}
    with Process(command, process.directory / "load", cpus=args.client_cpus,
                 peer_settings={"workers": args.client_workers}) as client:
        deadline = time.monotonic() + lifecycle_seconds(args.cycles, args.duration, timeout_ms)
        while client.child.poll() is None:
            if time.monotonic() >= deadline:
                raise TimeoutError("long connection client exceeded its bounded lifecycle")
            progress_before, _ = parse_progress(client.text(), args.cycles)
            first = len(process.samples)
            process.sample()
            process.check()
            if process.child.poll() is not None:
                raise RuntimeError("server exited during long connection load")
            progress_after, _ = parse_progress(client.text(), args.cycles)
            if phase := stable_phase(progress_before, progress_after):
                phases[phase].extend(process.samples[first:])
            client.sample()
            time.sleep(args.interval)
        client.check()
        progress, result = parse_progress(client.text(), args.cycles)
        if progress != phase_sequence(args.cycles):
            raise ValueError("long connection client ended before all phases")
        validate_result(result, protocol, connections, args.cycles)
        # Connection setup/close can finish between polls. Only the two explicit
        # hold windows require observations; retain setup samples when present.
        observations = []
        for (cycle, phase), samples in phases.items():
            if phase in ("connected", "disconnected") and len(samples) < 2:
                raise ValueError(f"sampling interval missed {cycle}/{phase} window")
            observations.append({"cycle": cycle, "phase": phase,
                                 "resources": summarize(samples) if samples else None,
                                 "last": samples[-1] if samples else None, "raw": samples})
        return {"result": result, "phases": observations,
                "client_resources": summarize(client.samples), "client_raw": client.samples}

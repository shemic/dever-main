"""Opt-in host SDK contract check. Pass only absolute runtime paths."""

import argparse
import json
from pathlib import Path
import struct
import subprocess
import tempfile
import threading
import time


ROOT = Path(__file__).resolve().parents[2]
HELLO = {
    "kind": "hello", "version": "dever-component-1", "port": "example.Text",
    "schema": "fixture-schema-v1", "adapter": "example.TextAdapter",
    "capabilities": [], "operations": ["text.render", "text.fail", "number.echo", "text.stubborn"],
    "setting": {"prefix": "hello "},
}


def frame(message):
    body = json.dumps(message, separators=(",", ":")).encode()
    return struct.pack(">I", len(body)) + body


def read_exact(stream, size):
    result = bytearray()
    while len(result) < size:
        part = stream.read(size - len(result))
        if not part:
            raise AssertionError("worker closed protocol stream")
        result.extend(part)
    return bytes(result)


def receive(process):
    deadline = threading.Timer(3, process.kill)
    deadline.start()
    try:
        size = struct.unpack(">I", read_exact(process.stdout, 4))[0]
        assert 0 < size <= 16 * 1024 * 1024
        return json.loads(read_exact(process.stdout, size))
    finally:
        deadline.cancel()


def send(process, message):
    process.stdin.write(frame(message))
    process.stdin.flush()


def run_case(command, *, keep_input_open=False):
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0) as process:
        try:
            send(process, HELLO)
            ready = receive(process)
            assert ready == {key: value for key, value in HELLO.items() if key != "setting"} | {"kind": "ready"}, ready
            send(process, {"kind": "health", "id": 0})
            assert receive(process) == {"kind": "health", "id": 0}
            send(process, {"kind": "call", "id": 1, "operation": "text.render", "payload": {"text": "world", "delay_ms": 0}})
            assert receive(process) == {"kind": "result", "id": 1, "payload": {"value": "hello world"}}
            send(process, {"kind": "call", "id": 2, "operation": "text.fail", "payload": {"text": "x", "delay_ms": 0}})
            assert receive(process) == {"kind": "error", "id": 2, "error": "example.rejected", "payload": {"reason": "rejected"}}
            send(process, {"kind": "call", "id": 3, "operation": "text.render", "payload": {"text": "late", "delay_ms": 500}})
            send(process, {"kind": "cancel", "id": 3})
            assert receive(process) == {"kind": "error", "id": 3, "error": "dever.cancelled", "payload": None}
            time.sleep(0.6)
            send(process, {"kind": "call", "id": 4, "operation": "text.render", "payload": {"text": "again", "delay_ms": 0}})
            assert receive(process) == {"kind": "result", "id": 4, "payload": {"value": "hello again"}}
            for request_id, number in enumerate((9007199254740993, 9223372036854775807, -9223372036854775808), 5):
                send(process, {"kind": "call", "id": request_id, "operation": "number.echo", "payload": {"number": number}})
                assert receive(process) == {"kind": "result", "id": request_id, "payload": {"number": number}}
            for request_id in (8, 9):
                send(process, {"kind": "call", "id": request_id, "operation": "text.render", "payload": {"text": "cancel", "delay_ms": 500}})
                send(process, {"kind": "cancel", "id": request_id})
                assert receive(process) == {"kind": "error", "id": request_id, "error": "dever.cancelled", "payload": None}
            send(process, {"kind": "shutdown"})
            assert receive(process) == {"kind": "shutdown"}
            # Python must finalize its reader without relying on the host's EOF timing.
            if not keep_input_open:
                process.stdin.close()
            assert process.wait(timeout=3) == 0, process.stderr.read().decode()
        except Exception as error:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)
            raise AssertionError(f"worker case failed: {error}; stderr={process.stderr.read().decode()}") from error
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)


def run_queued_shutdown(command):
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0) as process:
        try:
            send(process, HELLO)
            assert receive(process)["kind"] == "ready"
            messages = [
                {"kind": "call", "id": 1, "operation": "text.render", "payload": {"text": "cancel", "delay_ms": 500}},
                {"kind": "shutdown"},
                *[{"kind": "health", "id": 0} for _ in range(4)],
            ]
            process.stdin.write(b"".join(frame(message) for message in messages))
            process.stdin.flush()
            assert receive(process) == {"kind": "shutdown"}
            assert process.wait(timeout=3) == 0, process.stderr.read().decode()
            assert process.stderr.read() == b"", "shutdown left task/transport diagnostics"
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)


def run_typed_case(command, *, keep_input_open=False):
    hello = {
        "kind": "hello", "version": "dever-component-1", "port": "example.Typed",
        "schema": "typed-fixture-v1", "adapter": "example.Managed",
        "capabilities": [], "operations": ["render"], "setting": {"prefix": "hello "},
    }
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0) as process:
        try:
            send(process, hello)
            assert receive(process)["kind"] == "ready"
            message = {"name": "Ada", "number": 9223372036854775807, "labels": ["a", "b"]}
            send(process, {"kind": "call", "id": 1, "operation": "render", "payload": {"message": message}})
            assert receive(process) == {"kind": "result", "id": 1, "payload": {"text": "hello Ada9223372036854775807"}}
            message["name"] = "reject"
            send(process, {"kind": "call", "id": 2, "operation": "render", "payload": {"message": message}})
            assert receive(process) == {"kind": "error", "id": 2, "error": "example.Rejected", "payload": {"reason": "rejected"}}
            send(process, {"kind": "shutdown"})
            assert receive(process) == {"kind": "shutdown"}
            if not keep_input_open:
                process.stdin.close()
            assert process.wait(timeout=3) == 0, process.stderr.read().decode()
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)

    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0) as process:
        try:
            send(process, hello)
            assert receive(process)["kind"] == "ready"
            send(process, {"kind": "call", "id": 1, "operation": "render", "payload": {
                "message": {"name": "Ada", "number": 9223372036854775808, "labels": []},
            }})
            process.stdin.close()
            assert process.wait(timeout=3) != 0
            assert process.stdout.read() == b""
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)


def run_rejection(command, body):
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
        try:
            process.stdin.write(struct.pack(">I", len(body)) + body)
            process.stdin.flush()
            process.stdin.close()
            assert process.wait(timeout=3) != 0
            assert process.stdout.read() == b""
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)


def run_protocol_rejection(command, body, declared_size=None):
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0) as process:
        try:
            send(process, HELLO)
            assert receive(process)["kind"] == "ready"
            process.stdin.write(struct.pack(">I", len(body) if declared_size is None else declared_size) + body)
            process.stdin.flush()
            process.stdin.close()
            assert process.wait(timeout=3) != 0
            assert process.stdout.read() == b""
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)


def run_unresponsive_handler(command, final_message):
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=0) as process:
        try:
            send(process, HELLO)
            assert receive(process)["kind"] == "ready"
            send(process, {"kind": "call", "id": 1, "operation": "text.stubborn", "payload": {"text": "late", "delay_ms": 0}})
            time.sleep(0.1)
            send(process, final_message)
            if final_message["kind"] == "cancel":
                send(process, {"kind": "call", "id": 2, "operation": "text.render", "payload": {"text": "new", "delay_ms": 0}})
                send(process, {"kind": "cancel", "id": 2})
                send(process, {"kind": "shutdown"})
            assert process.wait(timeout=3) != 0
            assert process.stdout.read() == b""
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=3)


def runtime(path):
    if path is None:
        return None
    executable = Path(path)
    if not executable.is_absolute() or not executable.is_file():
        raise SystemExit(f"runtime path must be an existing absolute file: {path}")
    return str(executable)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--python")
    parser.add_argument("--node")
    parser.add_argument("--go")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="dever-sdk-check-") as directory:
        selected = {
            "Python": [runtime(args.python), "-B", str(ROOT / "sdk/python/example.py")] if args.python else None,
            "JavaScript": [runtime(args.node), str(ROOT / "sdk/javascript/example.mjs")] if args.node else None,
        }
        if args.go:
            go = runtime(args.go)
            binary = str(Path(directory) / "go-worker")
            build_environment = {
                "GOCACHE": str(Path(directory) / "go-cache"),
                "GOPATH": str(Path(directory) / "go-path"),
                "GOENV": "off", "GOTOOLCHAIN": "local", "GOPROXY": "off", "GOSUMDB": "off",
            }
            subprocess.run([go, "build", "-o", binary, "./example"], cwd=ROOT / "sdk/go", env=build_environment, check=True)
            selected["Go"] = [binary]
        else:
            selected["Go"] = None
        manifest = str(ROOT / "test/external-sdk/typed_contract.json")
        typed = {
            "Python": [runtime(args.python), "-B", str(ROOT / "test/external-sdk/typed_worker.py"), manifest] if args.python else None,
            "JavaScript": [runtime(args.node), str(ROOT / "test/external-sdk/typed_worker.mjs"), manifest] if args.node else None,
        }
        if args.go:
            typed_binary = str(Path(directory) / "go-typed-worker")
            subprocess.run([go, "build", "-o", typed_binary, str(ROOT / "test/external-sdk/typed_worker.go")], cwd=ROOT / "sdk/go", env=build_environment, check=True)
            typed["Go"] = [typed_binary, manifest]
        else:
            typed["Go"] = None
        for language, command in selected.items():
            if command is None:
                print(f"{language}: unavailable (no absolute runtime path supplied)")
                continue
            run_case(command, keep_input_open=language == "Python")
            if language == "Python":
                run_queued_shutdown(command)
            run_rejection(command, json.dumps({**HELLO, "schema": "wrong"}).encode())
            run_rejection(command, (json.dumps(HELLO)[:-1] + ',"schema":"duplicate"}').encode())
            run_rejection(command, (json.dumps(HELLO)[:-1] + ',"extra":true}').encode())
            run_rejection(command, b"\xff")
            call = {"kind": "call", "id": 1, "operation": "text.render", "payload": {"text": "x", "delay_ms": 0}}
            run_protocol_rejection(command, json.dumps({**call, "id": 2}).encode())
            run_protocol_rejection(command, json.dumps({**call, "operation": "unknown"}).encode())
            run_protocol_rejection(command, (json.dumps(call)[:-2] + ',"text":"duplicate"}}').encode())
            run_protocol_rejection(command, (json.dumps(call)[:-1] + ',"extra":true}').encode())
            run_protocol_rejection(command, b'{"kind":"call","id":1,"operation":"text.render","payload":NaN}')
            run_protocol_rejection(command, b'{"kind":"call","id":1,"operation":"text.render","payload":1e999}')
            nested = 0
            for _ in range(64):
                nested = [nested]
            run_protocol_rejection(command, json.dumps({**call, "payload": nested}).encode())
            run_protocol_rejection(command, json.dumps({**call, "payload": [0] * 65_537}).encode())
            run_protocol_rejection(command, b"", declared_size=16 * 1024 * 1024 + 1)
            run_unresponsive_handler(command, {"kind": "cancel", "id": 1})
            run_unresponsive_handler(command, {"kind": "shutdown"})
            run_typed_case(typed[language], keep_input_open=language == "Python")
            print(f"{language}: handshake, health, result, business error, cancel, shutdown and rejection passed")


if __name__ == "__main__":
    main()

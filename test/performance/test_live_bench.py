"""用独立 Python 对端检查 Rust 长连接客户端的正文与生命周期判定。"""

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import socketserver
import subprocess
import tempfile
import threading
import time
import unittest

from live import parse_progress, phase_sequence, validate_result
from settings import configured_peer, stage_peer


@contextmanager
def serving(server):
    with server:
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server.server_address[1]
        finally:
            server.shutdown()
            thread.join(timeout=2)
            if thread.is_alive():
                raise RuntimeError("owned test listener did not stop")


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(2)
        while payload := self.request.recv(4096):
            response = b"!" * len(payload) if self.server.wrong_body else payload
            self.request.sendall(response)
            if self.server.wrong_body:
                return


class InvalidEvent(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        body = b"event: live\nid: 1\ndata: wrong\n\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            # The client aborts sibling sessions after the first deliberate mismatch.
            return


class SilentEvent(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        self.wfile.write(b"event: live\nid: 1\ndata: ready\n\n")
        self.wfile.flush()
        time.sleep(1)


class LiveClientTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="dever-live-client-")
        self.addCleanup(directory.cleanup)
        self.executable = stage_peer(configured_peer("live_peer"), Path(directory.name), {"workers": 1})

    def execute(self, protocol, port, path):
        scheme = "http" if protocol == "sse" else protocol
        return subprocess.run([self.executable, protocol, f"{scheme}://127.0.0.1:{port}{path}", "2", "2", "300", "1000"],
                              capture_output=True, text=True, timeout=8)

    def test_tcp_reconnect_and_exact_echo(self):
        listener = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Echo)
        listener.wrong_body = False
        with serving(listener) as port:
            result = self.execute("tcp", port, "/")
        self.assertEqual(result.returncode, 0, result.stderr)
        phases, counts = parse_progress(result.stdout, 2)
        self.assertEqual(phases, phase_sequence(2))
        validate_result(counts, "tcp", 2, 2)

    def test_tcp_wrong_body_cannot_be_counted_as_held(self):
        listener = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Echo)
        listener.wrong_body = True
        with serving(listener) as port:
            result = self.execute("tcp", port, "/")
        self.assert_failure(result)

    def test_sse_wrong_event_cannot_pass_connection_readiness(self):
        with serving(ThreadingHTTPServer(("127.0.0.1", 0), InvalidEvent)) as port:
            result = self.execute("sse", port, "/events")
        self.assert_failure(result)

    def test_sse_silent_hold_cannot_pass_heartbeat_validation(self):
        with serving(ThreadingHTTPServer(("127.0.0.1", 0), SilentEvent)) as port:
            result = self.execute("sse", port, "/events")
        self.assertNotEqual(result.returncode, 0)
        counts = json.loads(result.stdout.splitlines()[-1])
        self.assertGreater(counts["errors"], 0)
        self.assertIn("PHASE|0|connected\n", result.stdout)
        self.assertNotIn("PHASE|0|closing\n", result.stdout)

    def assert_failure(self, result):
        self.assertNotEqual(result.returncode, 0)
        counts = json.loads(result.stdout.splitlines()[-1])
        self.assertGreater(counts["errors"], 0)
        self.assertNotIn("PHASE|0|connected\n", result.stdout)


if __name__ == "__main__":
    unittest.main()

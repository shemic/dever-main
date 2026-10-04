"""用独立的 Python HTTP 对端验证负载计数；只占用自有随机回环端口。"""

from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, HTTPServer
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest

from process import Process
from run import validate_load
from settings import configured_peer, stage_peer


ROOT = Path(__file__).resolve().parents[2]
TLS = ROOT / "test/dever-tests/fixtures/tls"


@contextmanager
def peer(body):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self):
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01})
    worker.start()
    try:
        yield server.server_port
    finally:
        server.shutdown()
        server.server_close()
        worker.join(timeout=2)
        if worker.is_alive():
            raise RuntimeError("owned HTTP peer did not stop")


@contextmanager
def benchmark_service(executable, mode):
    settings = {"workers": 1, "connections": 4, "task_capacity": 128,
                "timeout_ms": 1000, "lifetime_ms": 5000, "http2_streams": 16,
                "http2_stream_window_bytes": 65535, "http2_connection_window_bytes": 262144,
                "cert_path": str(TLS / "server.pem"), "key_path": str(TLS / "server-key.pem")}
    with tempfile.TemporaryDirectory(prefix="dever-h2-bench-") as directory:
        with Process([executable, mode], Path(directory) / "server", peer_settings=settings) as process:
            yield process.ready()
            process.check()


class NetworkMeasurementTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="dever-network-client-")
        self.addCleanup(directory.cleanup)
        self.executable = stage_peer(configured_peer("network_peer"), Path(directory.name), {"workers": 1})

    def load(self, port):
        executable = self.executable
        output = subprocess.run([str(executable), "load", "h1", f"http://localhost:{port}",
                                 "/plain", "20", "0.2", "1", "1", "500", "true", "-"],
                                capture_output=True, text=True, timeout=5, check=True)
        self.assertEqual(output.stderr, "")
        result = json.loads(output.stdout)
        validate_load(result)
        self.assertEqual(result["scheduled"], 4)
        self.assertGreaterEqual(result["elapsed_seconds"], 0.2)
        return result

    def test_success_requires_exact_body_and_full_window(self):
        with peer(b"hello") as port:
            result = self.load(port)
        self.assertEqual(result["succeeded"], 4)
        self.assertEqual(result["errors"], 0)
        self.assertEqual(result["dropped"], 0)
        self.assertEqual(result["latency_samples"], 4)
        self.assertEqual(result["http_version"], "h1")
        self.assertEqual(result["physical_connections"], 1)
        self.assertEqual(result["streams_per_connection"], 1)
        self.assertEqual(result["request_slots"], 1)

    def test_wrong_body_is_an_error_with_no_success_latency(self):
        with peer(b"wrong") as port:
            result = self.load(port)
        self.assertEqual(result["succeeded"], 0)
        self.assertEqual(result["errors"], 4)
        self.assertEqual(result["latency_samples"], 0)
        self.assertIsNone(result["latency_ms"])

    def test_http2_uses_one_physical_connection_for_sixteen_request_slots(self):
        executable = self.executable
        for mode, transport in (("runtime-http2", "http"), ("hyper-http2", "http"),
                                ("runtime-https2", "https"), ("hyper-https2", "https")):
            with self.subTest(mode=mode), benchmark_service(executable, mode) as port:
                output = subprocess.run(
                    [str(executable), "load", "h2", f"{transport}://localhost:{port}",
                     "/bytes", "200", "0.1", "1", "16", "1000", "true", str(TLS / "root.pem")],
                    capture_output=True, text=True, timeout=5, check=True)
                self.assertEqual(output.stderr, "")
                result = json.loads(output.stdout)
                validate_load(result)
                self.assertEqual(result["errors"], 0)
                self.assertGreater(result["succeeded"], 0)
                self.assertEqual(result["physical_connections"], 1)
                self.assertEqual(result["streams_per_connection"], 16)
                self.assertEqual(result["request_slots"], 16)
                self.assertEqual(result["successful_handshakes"], 1)


if __name__ == "__main__":
    unittest.main()

"""验证共享分配账本：线程缓存正常析构，真实泄漏和错误结果必须拒绝。"""

from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / "test/native-runtime-abi/managed-driver.c"
FIXTURE = ROOT / "test/native-runtime-abi/driver-fixture.c"
WRAPPED = ("malloc", "calloc", "realloc", "free", "posix_memalign", "realpath")


class DriverTests(unittest.TestCase):
    def run_fixture(self, scenario):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / "ledger"
            subprocess.run([
                "/usr/bin/cc", "-std=c11", "-Wall", "-Wextra", "-Werror", "-O2",
                f"-DDEVER_LEDGER_SCENARIO={scenario}", str(DRIVER), str(FIXTURE),
                *[f"-Wl,--wrap={name}" for name in WRAPPED], "-pthread", "-B/usr/bin/",
                "-o", str(executable),
            ], check=True, env={}, capture_output=True, text=True, timeout=20)
            return subprocess.run([str(executable)], env={}, capture_output=True,
                                  text=True, timeout=5)

    def test_late_thread_cache_is_released_before_counting(self):
        result = self.run_fixture(0)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_owned_leaks_and_changed_results_fail(self):
        for scenario, message in ((1, "allocation imbalance: 1"),
                                  (2, "changed its result on repeated execution: 7")):
            with self.subTest(scenario=scenario):
                result = self.run_fixture(scenario)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)


if __name__ == "__main__":
    unittest.main()

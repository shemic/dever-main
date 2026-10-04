"""Linux 进程与可选专属 cgroup 的生命周期和采样。"""

import os
from pathlib import Path
import signal
import statistics
import subprocess
import time
import uuid

from settings import stage_peer


def key_values(path):
    return {fields[0].rstrip(":"): fields[1] for line in path.read_text().splitlines()
            if len(fields := line.split()) >= 2}


def process_sample(pid):
    root = Path("/proc") / str(pid)
    status = key_values(root / "status")
    if "VmRSS" not in status:
        raise ProcessLookupError(f"process {pid} exited during sampling")
    memory = key_values(root / "smaps_rollup")
    # comm 字段可含空格和括号；从最后一个右括号之后解析固定 stat 字段。
    stat = (root / "stat").read_text().rsplit(")", 1)[1].split()
    switches = {"voluntary_ctxt_switches": 0, "nonvoluntary_ctxt_switches": 0}
    for thread in (root / "task").iterdir():
        try:
            thread_status = key_values(thread / "status")
        except FileNotFoundError:
            continue  # A completed blocking thread can disappear during a sample.
        for name in switches:
            switches[name] += int(thread_status[name])
    return {
        "time": time.monotonic(),
        "rss_bytes": int(status["VmRSS"]) * 1024,
        "rss_high_water_bytes": int(status["VmHWM"]) * 1024,
        "pss_bytes": int(memory["Pss"]) * 1024,
        "threads": int(status["Threads"]),
        "fds": len(list((root / "fd").iterdir())),
        "cpu_seconds": (int(stat[11]) + int(stat[12])) / os.sysconf("SC_CLK_TCK"),
        **switches,
    }


class Budget:
    def __init__(self, parent, memory_mib, cpu_quota):
        self.path = None
        self.final = None
        if parent is None:
            if memory_mib is not None or cpu_quota is not None:
                raise ValueError("memory/cpu quotas require --cgroup-parent")
            return
        self.path = parent.resolve() / ("dever-perf-" + uuid.uuid4().hex)
        self.path.mkdir()
        try:
            if memory_mib is not None:
                (self.path / "memory.max").write_text(str(memory_mib * 1024 * 1024))
                (self.path / "memory.swap.max").write_text("0")
            if cpu_quota is not None:
                (self.path / "cpu.max").write_text(f"{round(cpu_quota * 100000)} 100000")
        except BaseException:
            self.path.rmdir()
            raise

    def sample(self):
        if self.path is None:
            return None
        return {
            "current_bytes": int((self.path / "memory.current").read_text()),
            "peak_bytes": int((self.path / "memory.peak").read_text()),
            "events": {k: int(v) for k, v in key_values(self.path / "memory.events").items()},
            "cpu": {k: int(v) for k, v in key_values(self.path / "cpu.stat").items()},
            "max": (self.path / "memory.max").read_text().strip(),
            "cpu_max": (self.path / "cpu.max").read_text().strip(),
        }

    def close(self):
        if self.path is not None:
            try:
                self.final = self.sample()
            except OSError as error:
                try:
                    self.path.rmdir()
                except OSError as cleanup_error:
                    error.add_note(f"cannot remove owned cgroup {self.path}: {cleanup_error}")
                raise
            else:
                self.path.rmdir()


class Process:
    def __init__(self, command, directory, *, peer_settings=None, cpus=None, budget=None):
        self.command = [str(arg) for arg in command]
        self.directory = directory
        self.budget = budget
        self.samples = []
        self.directory.mkdir(parents=True, exist_ok=False)
        if peer_settings is not None:
            self.command[0] = str(stage_peer(command[0], directory, peer_settings))
        self.stdout = (directory / "stdout.log").open("w")
        self.stderr = (directory / "stderr.log").open("w")
        self.started = time.monotonic()

        def prepare_child():
            # 运行器单线程；只对即将 exec 的自有子进程设置 CPU/cgroup。
            if cpus is not None:
                os.sched_setaffinity(0, cpus)
            if budget is not None and budget.path is not None:
                (budget.path / "cgroup.procs").write_text(str(os.getpid()))

        try:
            self.child = subprocess.Popen(self.command, stdout=self.stdout, stderr=self.stderr,
                                          preexec_fn=prepare_child, start_new_session=True)
        except BaseException:
            self.stdout.close()
            self.stderr.close()
            raise

    def text(self):
        return (self.directory / "stdout.log").read_text()

    def descriptors(self):
        descriptors = {}
        for path in (Path("/proc") / str(self.child.pid) / "fd").iterdir():
            try:
                descriptors[path.name] = os.readlink(path)
            except FileNotFoundError:
                continue  # A descriptor may close while /proc is being read.
        return descriptors

    def sample(self):
        if self.child.poll() is not None:
            return
        try:
            sample = process_sample(self.child.pid)
        except (FileNotFoundError, ProcessLookupError):
            # Linux can release /proc memory fields before waitpid reports exit.
            # This observation is unavailable; check() still owns exit status.
            return
        sample["elapsed_seconds"] = sample.pop("time") - self.started
        if self.budget is not None:
            sample["cgroup"] = self.budget.sample()
        self.samples.append(sample)

    def ready(self, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.sample()
            for line in self.text().splitlines():
                if line.startswith("ERROR|"):
                    raise RuntimeError(line)
                if line.startswith("READY|"):
                    self.ready_seconds = time.monotonic() - self.started
                    return int(line.split("|")[1])
            if self.child.poll() is not None:
                raise RuntimeError(f"process exited before readiness: {self.directory}")
            time.sleep(0.005)
        raise TimeoutError(f"readiness timeout: {self.directory}")

    def check(self):
        code = self.child.poll()
        if code is not None and code != 0:
            raise RuntimeError(f"process exited {code}: {self.directory}")
        if "ERROR|" in self.text() or (self.directory / "stderr.log").stat().st_size:
            raise RuntimeError(f"benchmark reported an error: {self.directory}")

    def close(self):
        try:
            if self.child.poll() is None:
                try:
                    os.killpg(self.child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    # 子进程可能恰好在 poll 与 signal 之间自然退出。
                    pass
                try:
                    self.child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(self.child.pid, signal.SIGKILL)
                    self.child.wait(timeout=3)
        finally:
            self.stdout.close()
            self.stderr.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def observe(processes, seconds, interval):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        for process in processes:
            process.sample()
            process.check()
        time.sleep(min(interval, max(0, deadline - time.monotonic())))


def summarize(samples):
    if not samples:
        raise ValueError("process ended before any resource sample")
    result = {"samples": len(samples)}
    for metric in ("rss_bytes", "pss_bytes", "threads", "fds"):
        values = sorted(sample[metric] for sample in samples)
        result[metric + "_median"] = values[len(values) // 2]
        result[metric + "_max"] = max(values)
    result["rss_high_water_bytes"] = max(sample["rss_high_water_bytes"] for sample in samples)
    elapsed = samples[-1]["elapsed_seconds"] - samples[0]["elapsed_seconds"]
    cpu = samples[-1]["cpu_seconds"] - samples[0]["cpu_seconds"]
    result["cpu_seconds_sampled"] = cpu
    result["cpu_cores_sampled"] = cpu / elapsed if elapsed > 0 else None
    for name in ("voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"):
        result[name + "_sampled"] = samples[-1][name] - samples[0][name]
    return result


def resource_trend(samples):
    """报告四段中位数与后半程增长，避免将启动分配误记为长期泄漏。"""
    if len(samples) < 4:
        raise ValueError("resource trend requires at least four observations")
    windows = [samples[len(samples) * index // 4:len(samples) * (index + 1) // 4]
               for index in range(4)]
    medians = [statistics.median(sample["rss_bytes"] for sample in window) for window in windows]
    elapsed = samples[-1]["elapsed_seconds"] - samples[0]["elapsed_seconds"]
    return {"observed_seconds": elapsed, "rss_quarter_medians_bytes": medians,
            "rss_late_growth_bytes": medians[-1] - medians[2],
            "rss_late_growth_bytes_per_second": (medians[-1] - medians[2]) / (elapsed / 4) if elapsed > 0 else None,
            "fds_first": samples[0]["fds"], "fds_last": samples[-1]["fds"]}

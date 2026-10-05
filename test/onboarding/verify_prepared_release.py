#!/usr/bin/env python3
"""Opt-in acceptance of the actual signed release in an owned Linux image."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import shutil
import signal
import subprocess
import time

WORKSPACE = Path(__file__).resolve().parents[2]
OS_LIBRARIES = (
    "libc.so.6", "libdl.so.2", "libm.so.6", "libpthread.so.0", "librt.so.1",
    "libutil.so.1", "libresolv.so.2", "ld-linux-x86-64.so.2",
)


def execute(arguments, timeout=180):
    completed = subprocess.run(
        [str(value) for value in arguments], env={}, capture_output=True,
        text=True, timeout=timeout, check=False,
    )
    if completed.returncode:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {arguments}\n"
            f"{completed.stdout}\n{completed.stderr}"
        )
    return completed.stdout.strip()


def response(output, data="Hello, Dever"):
    expected = {"code": 0, "message": "ok", "data": data}
    if json.loads(output) != expected:
        raise RuntimeError(f"unexpected application response: {output}")


def minimal_os(image):
    libraries = image / "lib/x86_64-linux-gnu"
    libraries.mkdir(parents=True)
    for name in OS_LIBRARIES:
        shutil.copy2(Path("/lib/x86_64-linux-gnu") / name, libraries / name)
    (image / "lib64").mkdir()
    shutil.copy2(libraries / "ld-linux-x86-64.so.2", image / "lib64/ld-linux-x86-64.so.2")
    for name in ("tmp", "proc", "dev"):
        (image / name).mkdir()


def worker_project(image, ecosystem):
    """Small no-Lib Adapter: preparation must still acquire its managed runtime."""
    entries = {
        "pip": ("py", "import gzip\n\nasync def probe(payload, setting):\n    return {\"okay\": gzip.decompress(gzip.compress(b'dever')) == b'dever'}\n"),
        "npm": ("mjs", "import {createHash} from 'node:crypto';\nexport async function probe() { return {okay:createHash('sha256').update('dever').digest().length === 32}; }\n"),
        "go": ("go", (WORKSPACE / "test/ecosystem-release/probe.go").read_text()),
    }
    extension, source = entries[ecosystem]
    root = image / f"worker-{ecosystem}"
    files = {
        "config/setting.json": "{}\n",
        "module/sample/worker/port.dever": "probe() (okay: Bool) fails app.ProbeResult\n",
        "module/sample/worker/app.dever": "type ProbeResult { error Unavailable(message: Text) }\nprobe() (okay: Bool) { okay = port.probe() }\n",
        "module/sample/worker/api.dever": "cmd probe = app.probe\n",
        "module/sample/worker/adapter.dever": f'external {ecosystem} "worker/probe.{extension}" {{}}\n',
        f"module/sample/worker/worker/probe.{extension}": source,
    }
    for relative, contents in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents)
    return root


def isolated(image, init):
    return [
        WORKSPACE / "target/sandbox-inputs/assets/bin/bwrap",
        "--unshare-all", "--die-with-parent", "--new-session", "--bind", image,
        "/", "--proc", "/proc", "--dev", "/dev", "--chdir", "/", "--ro-bind",
        init, "/fixture-proc-init",
    ]


class InstalledDaemon:
    """All clients enter the same PID namespace so SO_PEERCRED stays valid."""

    def __init__(self, image, records, init):
        self.image = image
        self.records = records
        self.init = init
        self.child = None
        self.pidfd = None
        self.pid = None

    def __enter__(self):
        self.records.mkdir(parents=True, exist_ok=True)
        self.information = (self.records / "namespace.json").open("w+")
        self.errors = (self.records / "daemon.log").open("w+")
        self.child = subprocess.Popen(
            [str(value) for value in isolated(self.image, self.init) + [
                "--info-fd", "1", "--", "/fixture-proc-init",
                "/opt/dever/bin/deverd", "--root", "/opt/dever",
            ]], env={}, stdout=self.information, stderr=self.errors,
        )
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                if self.child.poll() is not None:
                    self.errors.seek(0)
                    raise RuntimeError(f"installed daemon exited: {self.errors.read()}")
                self.information.seek(0)
                raw = self.information.read()
                if raw.strip():
                    info = json.loads(raw)
                    if self.pidfd is None:
                        self.pid = info["child-pid"]
                        self.pidfd = os.pidfd_open(self.pid)
                    if (self.image / "run/dever/deverd.sock").exists():
                        return self
                time.sleep(0.02)
            raise RuntimeError("installed daemon readiness timed out")
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def command(self, *arguments):
        if self.child.poll() is not None:
            raise RuntimeError("owned daemon exited")
        expected = self.image.stat()
        actual = Path(f"/proc/{self.pid}/root").stat()
        if (expected.st_dev, expected.st_ino) != (actual.st_dev, actual.st_ino):
            raise RuntimeError("owned namespace identity changed")
        return execute([
            "/usr/bin/nsenter", "--target", str(self.pid), "--all", "--root",
            "--wdns=/", "--", "/usr/local/bin/dever", *arguments,
        ])

    def __exit__(self, *_):
        try:
            if self.pidfd is not None:
                try:
                    signal.pidfd_send_signal(self.pidfd, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                poller = select.poll()
                poller.register(self.pidfd, select.POLLIN)
                if not poller.poll(10000):
                    raise RuntimeError("owned namespace did not terminate")
            elif self.child is not None and self.child.poll() is None:
                self.child.kill()
            if self.child is not None:
                self.child.wait(timeout=10)
        finally:
            if self.pidfd is not None:
                os.close(self.pidfd)
                self.pidfd = None
            self.information.close()
            self.errors.close()


def ordinary_uid_target_reuse(image):
    # bwrap's single-UID user namespace cannot represent a second identity.
    # Use the installed private machine with real kernel UIDs for this check;
    # no host language tools or global service participate in target reuse.
    machine = image / "opt/dever"
    child = subprocess.Popen([str(machine / "bin/deverd"), "--root", str(machine)],
                             env={}, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        deadline = time.monotonic() + 10
        while not (machine / "state/deverd.sock").exists():
            if child.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("private UID acceptance daemon did not become ready")
            time.sleep(0.02)
        for uid in (65533, 65534):
            completed = subprocess.run(
                [str(machine / "bin/dever"), "target", "add", "linux-aarch64"],
                env={}, user=uid, group=uid, extra_groups=[], capture_output=True,
                text=True, timeout=180, check=False,
            )
            if completed.returncode:
                raise RuntimeError(f"UID {uid} target reuse failed: {completed.stderr}")
    finally:
        if child.poll() is None:
            child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=10)
        child.stderr.close()


def install_assets(assets, image, manifest):
    # Only transport is local: signature verification, full tar extraction and
    # the final bootstrap executable are the real first-install implementation.
    script = WORKSPACE / "skills/scripts/install.py"
    specification = importlib.util.spec_from_file_location("dever_release_installer", script)
    installer = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(installer)
    stem = f"dever-{manifest['platform']}"
    base = installer.RELEASES
    routes = {
        f"{base}/latest/download/{stem}.manifest.json": assets / f"{stem}.manifest.json",
        f"{base}/latest/download/{stem}.manifest.sig": assets / f"{stem}.manifest.sig",
        f"{base}/download/v{manifest['version']}/{stem}.tar.zst": assets / f"{stem}.tar.zst",
    }
    for artifact in manifest["artifacts"]:
        if artifact["path"] == "bootstrap/dever" or artifact["path"].startswith("bootstrap/lib/"):
            name = f"{stem}.blob-{artifact['sha256']}"
            routes[f"{base}/download/v{manifest['version']}/{name}"] = assets / name
    installer.download = lambda address: routes[address].open("rb")
    installer.install(argparse.Namespace(
        version="latest", release=None, system_root=image, no_start=True,
        trusted_key=WORKSPACE / "skills/assets/release.pub",
    ))


def accept(release, assets, image, report, init, extension_helper=None):
    image.mkdir(mode=0o755)
    report.parent.mkdir(parents=True, exist_ok=True)
    manifest = json.loads((release / "manifest.json").read_bytes())
    result = {
        "version": manifest["version"], "platform": manifest["platform"],
        "manifest_sha256": hashlib.sha256((release / "manifest.json").read_bytes()).hexdigest(),
        "checks": [], "global_service_actions": "not_run", "online_update": "not_run",
        "installation_transport": "local_github_assets" if assets else "local_signed_directory",
    }

    def passed(name):
        result["checks"].append(name)
        report.write_text(json.dumps(result, indent=2) + "\n")
        print(f"PASS {name}", flush=True)

    try:
        minimal_os(image)
        if assets:
            install_assets(assets, image, manifest)
        else:
            execute([
                "/usr/bin/python3", WORKSPACE / "skills/scripts/install.py",
                "--release", release, "--version", manifest["version"],
                "--system-root", image,
            ])
        installed = image / "opt/dever/versions" / manifest["version"]
        if (installed / "manifest.json").read_bytes() != (release / "manifest.json").read_bytes():
            raise RuntimeError("installed package differs from the release under acceptance")
        passed("actual_signed_release_first_install")
        for extension in manifest["extensions"]:
            if any((installed / artifact["path"]).exists() for artifact in extension["artifacts"]):
                raise RuntimeError("base install unexpectedly includes optional extension bytes")
        passed("base_install_excludes_optional_extensions")
        launcher = image / "opt/dever/bin/dever"
        if execute([launcher, "version"]) != manifest["version"]:
            raise RuntimeError("installed version mismatch")
        if (image / "usr/local/bin/dever").readlink() != Path("/opt/dever/bin/dever"):
            raise RuntimeError("public launcher link mismatch")
        passed("installed_version_and_public_entry")
        standalone = [("first", "hello.greeting.greet", '{"name":"Dever"}', "Hello, Dever"),
                      ("second", "hello.greeting.greet", '{"name":"Dever"}', "Hello, Dever")]
        with InstalledDaemon(image, report.parent / f"{report.stem}-process", init) as daemon:
            for project, markdown in (("first", False), ("second", True)):
                root = "/" + project
                args = ["new", root] + (["--markdown"] if markdown else [])
                daemon.command(*args)
                daemon.command("fmt", root, "--check")
                daemon.command("check", root)
                test_result = daemon.command("test", root)
                if "test result: ok. 1 passed; 0 failed; 0 not run" not in test_result:
                    raise RuntimeError(f"missing application test success: {test_result}")
                response(daemon.command("run", root, "--", "hello.greeting.greet", '{"name":"Dever"}'))
                daemon.command("build", root, "--output", root + "/program")
                passed(f"{'markdown' if markdown else 'plain'}_new_fmt_check_test_run_build")
            expected = f"/opt/dever/versions/{manifest['version']}/skills/dever-language"
            if daemon.command("skill", "path") != expected:
                raise RuntimeError("versioned skill did not match installed core")
            daemon.command("skill", "install", "/ai-skill")
            if not (image / "ai-skill/SKILL.md").is_file():
                raise RuntimeError("missing installed skill entry")
            passed("versioned_skill_resolution_and_loader_install")
            try:
                daemon.command("build", "/first", "--target", "linux-aarch64", "--output", "/first/program.arm64")
            except RuntimeError as error:
                if "dever target add linux-aarch64" not in str(error):
                    raise
            else:
                raise RuntimeError("unprepared cross build should fail without fetching resources")
            passed("missing_target_fails_offline_with_preparation_command")
            if assets and extension_helper:
                execute([extension_helper, image / "opt/dever", assets, manifest["version"], "target-linux-aarch64"])
                daemon.command("target", "add", "linux-aarch64")
                daemon.command("build", "/first", "--target", "linux-aarch64", "--output", "/first/program.arm64")
                with (image / "first/program.arm64").open("rb") as executable:
                    header = executable.read(20)
                if header[:4] != b"\x7fELF" or header[18:20] != b"\xb7\x00":
                    raise RuntimeError("cross build did not produce an ARM64 ELF")
                passed("actual_target_extension_install_reuse_and_arm64_application_cross_build")
                for ecosystem in ("pip", "npm", "go"):
                    execute([extension_helper, image / "opt/dever", assets, manifest["version"],
                             f"runtime-{ecosystem}-{manifest['platform']}"])
                    project = worker_project(image, ecosystem)
                    root = "/" + project.name
                    daemon.command("lib", "update", root)
                    before = [(project / name).read_bytes() for name in ("config/setting.json", "dever.lock")]
                    daemon.command("lib", "install", root)
                    after = [(project / name).read_bytes() for name in ("config/setting.json", "dever.lock")]
                    if before != after:
                        raise RuntimeError("exact Lib installation modified project configuration or lock")
                    daemon.command("check", root)
                    response(daemon.command("run", root, "--", "sample.worker.probe", "{}"), True)
                    daemon.command("build", root, "--output", root + "/program")
                    standalone.append((project.name, "sample.worker.probe", "{}", True))
                    passed(f"actual_{ecosystem}_extension_no_lib_worker_exact_install_run_build")
            result["cache"] = json.loads(daemon.command("cache", "status"))
        if assets and extension_helper:
            ordinary_uid_target_reuse(image)
            passed("two_ordinary_uids_reuse_shared_signed_target_extension")
        # Remove sources and the entire machine from the visible OS root. Keep
        # them outside that root for a subsequent online-update acceptance.
        moved = []
        try:
            for relative in ["opt/dever"] + [f"{project}/module" for project, *_ in standalone]:
                source = image / relative
                hidden = image.parent / ("hidden-" + relative.replace("/", "-"))
                source.rename(hidden)
                moved.append((source, hidden))
            for project, entry, parameters, data in standalone:
                # These projects were created above; remove only their generated
                # Worker cache so standalone execution must extract its own bundle.
                cache = image / project / "data/cache/lib"
                if cache.exists():
                    shutil.rmtree(cache)
                response(execute(isolated(image, init) + [
                    "--", "/fixture-proc-init", f"/{project}/program",
                    entry, parameters,
                ]), data)
            passed("standalone_programs_without_sources_or_machine")
        finally:
            for source, hidden in reversed(moved):
                hidden.rename(source)
        result["status"] = "passed"
    except BaseException as error:
        result["status"] = "failed"
        result["error"] = str(error)
        raise
    finally:
        report.write_text(json.dumps(result, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", required=True, type=Path)
    parser.add_argument("--assets", type=Path, help="exercise actual split download assets")
    parser.add_argument("--extension-helper", type=Path, help="explicit prepare-extensions author fixture")
    parser.add_argument("--init", type=Path, required=True, help="explicit static native-acceptance-init fixture")
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("requires root for an owned image/namespace; never installs host services")
    accept(args.release.resolve(), args.assets.resolve() if args.assets else None,
           args.image.absolute(), args.report.absolute(), args.init.absolute(),
           args.extension_helper.absolute() if args.extension_helper else None)


if __name__ == "__main__":
    main()

"""用显式准备的私有 ARM 内核运行独立应用，不改主机服务或沙箱策略。"""
from pathlib import Path
import json
import shutil
import subprocess
import time

SOURCE = Path(__file__).resolve().parent
ROOT = SOURCE.parents[2] / "target/arm64-cross"


def prepare_images():
    init = ROOT / "initramfs"
    for source, destination in [
        ("init.sh", init / "init-arm"),
        ("bootstrap.sh", init / "init-stage"),
        ("guest.sh", ROOT / "guest/run.sh"),
    ]:
        shutil.copyfile(SOURCE / source, destination)
        destination.chmod(0o755)
    archive = ROOT / "guest-initramfs.gz"
    with archive.open("wb") as output:
        with subprocess.Popen(
            ["/usr/bin/find", ".", "-print0"], cwd=init, stdout=subprocess.PIPE
        ) as find:
            try:
                with subprocess.Popen(
                    ["/usr/bin/cpio", "--null", "-o", "--format=newc", "--quiet"],
                    cwd=init, stdin=find.stdout, stdout=subprocess.PIPE,
                ) as cpio:
                    find.stdout.close()
                    try:
                        subprocess.run(["/usr/bin/gzip", "-1"], stdin=cpio.stdout, stdout=output, check=True)
                    finally:
                        cpio.stdout.close()
            finally:
                find.stdout.close()
        if find.returncode or cpio.returncode:
            raise RuntimeError("initramfs creation failed")
    disk = ROOT / "guest.ext4"
    with disk.open("wb") as output:
        output.truncate(1024 * 1024 * 1024)
    subprocess.run(
        ["/usr/sbin/mke2fs", "-q", "-t", "ext4", "-F", "-d", str(ROOT / "guest"), str(disk)],
        check=True,
    )
    return archive, disk


def main():
    archive, disk = prepare_images()
    command = [
        "/lib64/ld-linux-x86-64.so.2", "--library-path",
        str(ROOT / "tools/usr/lib/x86_64-linux-gnu"),
        str(ROOT / "tools/usr/bin/qemu-system-aarch64"),
        "-machine", "virt", "-cpu", "cortex-a72", "-smp", "2", "-m", "768",
        "-nographic", "-no-reboot", "-nic", "none",
        "-kernel", str(ROOT / "iso/boot/vmlinuz-virt"), "-initrd", str(archive),
        "-append", "console=ttyAMA0 rdinit=/init-arm panic=-1",
        "-drive", f"file={disk},format=raw,if=none,readonly=on,id=payload",
        "-device", "virtio-blk-device,drive=payload",
    ]
    log = ROOT / "logs" / f"guest-{time.time_ns()}.log"
    started = time.monotonic()
    timed_out = False
    with log.open("xb") as output:
        try:
            result = subprocess.run(command, stdout=output, stderr=subprocess.STDOUT, timeout=600)
            exit_code = result.returncode
        except subprocess.TimeoutExpired:
            timed_out = True
            exit_code = None
    transcript = log.read_text(errors="replace")
    required = ["DEVER_ARM_GUEST_EXIT=0", "ARM_HTTP_PASS"]
    required.extend(f"ARM_WORKER_PASS={ecosystem}" for ecosystem in ("pip", "npm", "go"))
    passed = exit_code == 0 and all(marker in transcript for marker in required)
    record = {
        "command": command, "exit_code": exit_code, "timed_out": timed_out, "passed": passed,
        "seconds": time.monotonic() - started, "log": str(log),
    }
    log.with_suffix(".json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record), flush=True)
    if not passed:
        raise RuntimeError("ARM guest acceptance failed; see serial log")


if __name__ == "__main__":
    main()

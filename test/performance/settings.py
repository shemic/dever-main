"""基准配置与隔离 peer 布置；配置只来自 config/setting.json。"""

import errno
import json
import os
from pathlib import Path
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[2]
PEER_CONFIGURATION = "setting-json-v1"


def write_private_json(path, value, *, replace=False):
    path = Path(path)
    temporary = None
    if replace:
        descriptor, temporary = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}-")
    else:
        descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "w") as output:
            os.fchmod(output.fileno(), 0o600)
            json.dump(value, output, indent=2)
            output.write("\n")
        if temporary is not None:
            os.replace(temporary, path)
    finally:
        if temporary is not None:
            Path(temporary).unlink(missing_ok=True)


def stage_peer(executable, directory, settings):
    """独立配置共用不可变程序；测量期间不得重写输入二进制。"""
    executable = Path(executable).resolve(strict=True)
    destination = directory / executable.name
    (directory / "config").mkdir()
    write_private_json(directory / "config/setting.json", {"benchmark": settings})
    try:
        os.link(executable, destination)
    except OSError as error:
        if error.errno != errno.EXDEV:
            raise
        # 配置目录可以位于另一文件系统；这种情况下硬链接不可用。
        with destination.open("xb") as output, executable.open("rb") as source:
            shutil.copyfileobj(source, output)
        shutil.copymode(executable, destination)
    return destination.resolve()


def configured_peer(name, setting_path=ROOT / "config/setting.json"):
    document = json.loads(setting_path.read_text())
    section = document.get("performance") if isinstance(document, dict) else None
    value = section.get(name) if isinstance(section, dict) else None
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"config/setting.json must define performance.{name} as a binary path")
    # 仓库设置中的相对路径以项目根目录为基准。
    path = Path(value)
    return (setting_path.parent.parent / path).resolve(strict=True)

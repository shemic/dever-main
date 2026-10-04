#!/usr/bin/env python3
"""Turn a prepared signed release into the three official GitHub release assets."""

import argparse
import ctypes
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import tarfile
import tempfile


def create(release, output):
    release = release.absolute()
    output = output.absolute()
    if release.is_symlink() or not release.is_dir():
        raise ValueError("release must be a real prepared directory")
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    if not output.parent.is_dir():
        raise ValueError("output parent must exist")
    manifest_bytes = (release / "manifest.json").read_bytes()
    manifest = json.loads(manifest_bytes)
    if manifest["format"] != "dever-release-v1" or manifest["platform"] not in ("linux-x86_64", "linux-aarch64"):
        raise ValueError("unsupported release format or platform")
    artifacts = manifest["artifacts"]
    if not 1 <= len(artifacts) <= 4096 or sum(entry["bytes"] for entry in artifacts) > 2 * 1024**3:
        raise ValueError("release exceeds the official asset limits")
    names = set()
    for artifact in artifacts:
        name = artifact["path"]
        if (not name or str(PurePosixPath(name)) != name or name.startswith("/")
                or any(part in ("", ".", "..") for part in name.split("/"))
                or any(character in name for character in ("\\", ":", "\0"))
                or name in names or name in ("manifest.json", "manifest.sig")):
            raise ValueError("invalid release artifact path")
        names.add(name)
        path = release / name
        for parent in (path, *path.parents):
            if parent.is_symlink():
                raise ValueError("release artifacts must not traverse symlinks")
            if parent == release:
                break
        if not path.is_file() or path.stat().st_size != artifact["bytes"]:
            raise ValueError("release artifact type/size mismatch")
        with path.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != artifact["sha256"]:
                raise ValueError("release artifact digest mismatch")
    with tempfile.TemporaryDirectory(prefix=".dever-release-assets-", dir=output.parent) as scratch:
        prepared = Path(scratch) / "assets"
        prepared.mkdir()
        stem = f"dever-{manifest['platform']}"
        (prepared / f"{stem}.manifest.json").write_bytes(manifest_bytes)
        (prepared / f"{stem}.manifest.sig").write_bytes((release / "manifest.sig").read_bytes())
        with (prepared / f"{stem}.tar.gz").open("xb") as compressed:
            with gzip.GzipFile(filename="", fileobj=compressed, mode="wb", mtime=0) as stream:
                with tarfile.open(fileobj=stream, mode="w|", format=tarfile.USTAR_FORMAT) as archive:
                    for artifact in sorted(artifacts, key=lambda entry: entry["path"]):
                        path = release / artifact["path"]
                        header = tarfile.TarInfo(artifact["path"])
                        header.size = artifact["bytes"]
                        header.mode = 0o644
                        with path.open("rb") as contents:
                            archive.addfile(header, contents)
        publish(prepared, output)
    return sorted(path.name for path in output.iterdir())


def publish(prepared, output):
    # Linux renameat2 publishes the entire owned tree without replacing even an
    # empty directory created by another publisher. Failure cleans only staging.
    libc = ctypes.CDLL("libc.so.6", use_errno=True)
    rename = libc.renameat2
    rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    rename.restype = ctypes.c_int
    if rename(-100, os.fsencode(prepared), -100, os.fsencode(output), 1) != 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error), str(output))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        for name in create(args.release, args.output):
            print(name)
    except (OSError, ValueError, KeyError, TypeError, tarfile.TarError) as error:
        parser.exit(1, f"cannot prepare release assets: {error}\n")


if __name__ == "__main__":
    main()

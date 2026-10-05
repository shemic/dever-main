#!/usr/bin/env python3
"""Publish base, optional extension and authenticated bootstrap assets from a signed release."""

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile


MAX_PACKAGE = 2 * 1024**3


def extension_suffix(extension):
    kind = extension["kind"]
    target = extension["target"]
    if target not in ("linux-x86_64", "linux-aarch64"):
        raise ValueError("unsupported extension target")
    if kind == {"type": "target"}:
        name = "target"
    elif (set(kind) == {"type", "ecosystem"}
          and kind["type"] in ("runtime", "build")
          and kind["ecosystem"] in ("pip", "npm", "go")
          and not (kind["type"] == "build" and kind["ecosystem"] == "go")):
        name = f"{kind['type']}-{kind['ecosystem']}"
    else:
        raise ValueError("unsupported extension kind")
    return f"ext-{name}-{target}"


def packages(manifest):
    if (set(manifest) != {"format", "version", "platform", "artifacts", "extensions"}
            or manifest["format"] != "dever-release-v2"
            or manifest["platform"] not in ("linux-x86_64", "linux-aarch64")
            or not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", manifest["version"])):
        raise ValueError("unsupported release format, version or platform")
    if not isinstance(manifest["extensions"], list) or len(manifest["extensions"]) > 12:
        raise ValueError("release extension count exceeds limit")
    selected = {"": manifest["artifacts"]}
    for extension in manifest["extensions"]:
        if set(extension) != {"kind", "target", "artifacts"}:
            raise ValueError("invalid extension fields")
        suffix = extension_suffix(extension)
        if suffix in selected:
            raise ValueError("duplicate extension identity")
        target = extension["target"]
        kind = extension["kind"]
        if kind["type"] == "target":
            if target == manifest["platform"]:
                raise ValueError("host native resources belong to the base package")
            prefixes = (f"runtime/{target}/", f"sandbox/{target}/")
        else:
            prefixes = (f"{kind['type']}/{kind['ecosystem']}/{target}/",)
        if any(not artifact["path"].startswith(prefixes) for artifact in extension["artifacts"]):
            raise ValueError("extension artifact is outside its resource namespace")
        selected[suffix] = extension["artifacts"]
    return selected


def verify_artifacts(release, groups):
    names = set()
    for artifacts in groups.values():
        if (not 1 <= len(artifacts) <= 4096
                or any(type(entry["bytes"]) is not int or entry["bytes"] < 0 for entry in artifacts)
                or sum(entry["bytes"] for entry in artifacts) > MAX_PACKAGE):
            raise ValueError("release exceeds the official asset limits")
        for artifact in artifacts:
            verify_artifact(release, artifact, names)
    for name in names:
        if any(str(parent) in names for parent in PurePosixPath(name).parents):
            raise ValueError("release artifacts overlap as file and directory")


def verify_artifact(release, artifact, names):
    name = artifact["path"]
    if (set(artifact) != {"path", "bytes", "sha256"}
            or not name or len(name.encode("utf-8")) > 1024
            or str(PurePosixPath(name)) != name or name.startswith("/")
            or any(part in ("", ".", "..") for part in name.split("/"))
            or any(character in name for character in ("\\", ":", "\0"))
            or name in names or name in ("manifest.json", "manifest.sig")
            or not re.fullmatch(r"[0-9a-f]{64}", artifact["sha256"])):
        raise ValueError("invalid release artifact path or digest")
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


def compress(release, artifacts, destination, zstd):
    # Compression is an explicit author tool; installation uses the signed launcher codec.
    with destination.open("xb") as output:
        process = subprocess.Popen([str(zstd), "-q", "-10", "--long=27", "-c"],
                                   stdin=subprocess.PIPE, stdout=output,
                                   stderr=subprocess.PIPE, env={})
        try:
            with process.stdin:
                with tarfile.open(fileobj=process.stdin, mode="w|", format=tarfile.USTAR_FORMAT) as archive:
                    for artifact in sorted(artifacts, key=lambda entry: entry["path"]):
                        header = tarfile.TarInfo(artifact["path"])
                        header.size = artifact["bytes"]
                        header.mode = 0o644
                        with (release / artifact["path"]).open("rb") as contents:
                            archive.addfile(header, contents)
            errors = process.stderr.read(4096)
            if process.wait() != 0:
                raise ValueError(f"release compression failed: {errors.decode(errors='replace')}")
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()
            process.stderr.close()


def create(release, output, zstd):
    release = release.absolute()
    output = output.absolute()
    if release.is_symlink() or not release.is_dir():
        raise ValueError("release must be a real prepared directory")
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    if not output.parent.is_dir():
        raise ValueError("output parent must exist")
    if not zstd.is_absolute() or not zstd.is_file():
        raise ValueError("zstd must be an explicit absolute author tool path")
    with (release / "manifest.json").open("rb") as stream:
        manifest_bytes = stream.read(2 * 1024**2 + 1)
    if len(manifest_bytes) > 2 * 1024**2:
        raise ValueError("release metadata exceeds limit")
    manifest = json.loads(manifest_bytes)
    groups = packages(manifest)
    verify_artifacts(release, groups)
    with tempfile.TemporaryDirectory(prefix=".dever-release-assets-", dir=output.parent) as scratch:
        prepared = Path(scratch) / "assets"
        prepared.mkdir()
        stem = f"dever-{manifest['platform']}"
        (prepared / f"{stem}.manifest.json").write_bytes(manifest_bytes)
        (prepared / f"{stem}.manifest.sig").write_bytes((release / "manifest.sig").read_bytes())
        for suffix, artifacts in groups.items():
            filename = f"{stem}{'.' + suffix if suffix else ''}.tar.zst"
            compress(release, artifacts, prepared / filename, zstd)
        for artifact in manifest["artifacts"]:
            if artifact["path"] == "bootstrap/dever" or artifact["path"].startswith("bootstrap/lib/"):
                blob = prepared / f"{stem}.blob-{artifact['sha256']}"
                if not blob.exists():
                    shutil.copyfile(release / artifact["path"], blob)
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
    parser.add_argument("--zstd", required=True, type=Path, help="explicit author-side zstd executable")
    args = parser.parse_args()
    try:
        for name in create(args.release, args.output, args.zstd):
            print(name)
    except (OSError, ValueError, KeyError, TypeError, tarfile.TarError) as error:
        parser.exit(1, f"cannot prepare release assets: {error}\n")


if __name__ == "__main__":
    main()

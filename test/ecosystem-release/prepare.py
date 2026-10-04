"""Prepare pinned, offline upstream inputs for the explicit release acceptance.

This test author tool never discovers interpreters or downloads anything. It
materializes selected distribution files and resolves only in-archive aliases.
The production release maker subsequently verifies every prepared input again.
"""

import argparse
import gzip
import hashlib
import json
import posixpath
import shutil
import tarfile
from pathlib import Path, PurePosixPath


WORKSPACE = Path(__file__).resolve().parents[2]
INPUTS = WORKSPACE / "target/ecosystem-release-inputs"
SOURCES = {
    "pip": (
        INPUTS / "downloads/cpython-3.12.14.tar.gz",
        "72748da13197c1fb161e3afeef20a6a385ff24f2165e6e2758e47008e7faba4c",
        "python/",
    ),
    "npm": (
        INPUTS / "downloads/node-v24.15.0-linux-x64.tar.xz",
        "472655581fb851559730c48763e0c9d3bc25975c59d518003fc0849d3e4ba0f6",
        "node-v24.15.0-linux-x64/",
    ),
    "go": (
        WORKSPACE / "target/go-managed/runtime.pack",
        "8818a1eccb510948a55f63088c01678391a5e0c2236e6eacadfe45d0a5edbb43",
        "",
    ),
}


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def relative_path(name):
    path = PurePosixPath(name)
    if path.is_absolute() or path.as_posix() != name or ".." in path.parts:
        raise ValueError(f"unsafe upstream archive path: {name}")
    return name


def selected(ecosystem, path):
    if ecosystem == "npm":
        return path in ("bin/node", "LICENSE")
    if ecosystem == "pip":
        # Keep the complete standard library and its native/data dependencies.
        # pip, user site-packages, development headers and command aliases are
        # not needed by the isolated runtime (-I -S -B).
        return path == "bin/python3.12" or (
            path.startswith(("lib/", "share/"))
            and not path.startswith("lib/python3.12/site-packages/")
        )
    return True


def materialize(ecosystem, destination, *, selector=None, source_descriptor=None):
    source, expected, prefix = source_descriptor or SOURCES[ecosystem]
    if digest(source) != expected:
        raise ValueError(f"{ecosystem} upstream input digest differs")
    aliases = {}
    seen = set()
    with tarfile.open(source, "r:*") as archive:
        for member in archive:
            name = relative_path(member.name.rstrip("/"))
            if not name.startswith(prefix) or member.isdir():
                continue
            path = name[len(prefix):]
            if not (selector(path) if selector is not None else selected(ecosystem, path)):
                continue
            if path in seen:
                raise ValueError(f"duplicate upstream path: {path}")
            seen.add(path)
            output = destination / path
            output.parent.mkdir(parents=True, exist_ok=True)
            if member.issym():
                target = posixpath.normpath(posixpath.join(posixpath.dirname(path), member.linkname))
                aliases[path] = relative_path(target)
            elif member.isfile():
                with archive.extractfile(member) as payload, output.open("xb") as target:
                    shutil.copyfileobj(payload, target)
                output.chmod(0o644)
            else:
                raise ValueError(f"unsupported upstream entry: {path}")
    while aliases:
        resolved = []
        for path, target in aliases.items():
            if (destination / target).is_file():
                shutil.copyfile(destination / target, destination / path)
                resolved.append(path)
        if not resolved:
            raise ValueError("upstream aliases escape the selected tree or contain a cycle")
        for path in resolved:
            del aliases[path]
    return {"sha256": expected, "bytes": source.stat().st_size}


def runtime_settings(root, ecosystem, *, target="linux-x86_64"):
    architecture = {"linux-x86_64": "x86_64", "linux-aarch64": "aarch64"}[target]
    destination = root / ecosystem
    versions = {"pip": ("cpython", "3.12.14"), "npm": ("node", "24.15.0"), "go": ("go", "1.26.3")}
    name, version = versions[ecosystem]
    python_markers = {
        "implementation_name": "cpython", "implementation_version": version,
        "os_name": "posix", "platform_machine": architecture,
        "platform_python_implementation": "CPython", "platform_release": "",
        "platform_system": "Linux", "platform_version": "",
        "python_full_version": version, "python_version": "3.12", "sys_platform": "linux",
    } if ecosystem == "pip" else None
    # This fixed CPython 3.12 author fixture uses a conservative glibc 2.36
    # compatibility ceiling. Its separately signed Ubuntu OS ABI pack currently
    # supplies glibc 2.39; admission checks actual ELF version requirements too.
    oldest_glibc = 4 if architecture == "x86_64" else 16
    platforms = [f"manylinux_2_{minor}_{architecture}" for minor in range(36, oldest_glibc, -1)]
    platforms += [f"manylinux2014_{architecture}"]
    if architecture == "x86_64":
        platforms += ["manylinux2010_x86_64", "manylinux1_x86_64"]
    platforms += [f"linux_{architecture}"]
    python_tags = [f"cp312-cp312-{platform}" for platform in platforms]
    python_tags += [f"cp3{minor}-abi3-{platform}" for minor in range(12, 1, -1) for platform in platforms]
    python_tags += [f"py3-none-{platform}" for platform in platforms]
    python_tags += ["py312-none-any", "py3-none-any"]
    if ecosystem != "go":
        manifest = {
            "format": "dever-worker-runtime-v1",
            "ecosystem": ecosystem,
            "target": target,
            "executable": "bin/python3.12" if ecosystem == "pip" else "bin/node",
            "arguments": ["-I", "-S", "-B"] if ecosystem == "pip" else [],
        }
        if ecosystem == "pip":
            manifest["python_wheel_tags"] = python_tags
            manifest["python_extension_suffixes"] = [f".cpython-312-{architecture}-linux-gnu.so", ".abi3.so", ".so"]
            manifest["python_markers"] = python_markers
        (destination / "dever-runtime.json").write_text(json.dumps(manifest), encoding="utf-8")
    files = [
        {"source": path.relative_to(root).as_posix(), "path": path.relative_to(destination).as_posix(), "sha256": digest(path)}
        for path in sorted(destination.rglob("*")) if path.is_file()
    ]
    settings = {"name": name, "version": version, "files": files}
    if ecosystem == "npm":
        settings["npm_libc"] = "glibc"
    if ecosystem == "pip":
        settings["python_markers"] = python_markers
        settings["python_wheel_tags"] = python_tags
    return settings


def worker_runtime(root, output, ecosystem):
    """Make one explicit fixture pack from verified leaves, without another tree copy."""
    settings = json.loads((root / "config/setting.json").read_text(encoding="utf-8"))["runtimes"][ecosystem]
    inputs = sorted(settings["files"], key=lambda entry: entry["path"])
    for entry in inputs:
        source = root / relative_path(entry["source"])
        if source.is_symlink() or not source.is_file() or digest(source) != entry["sha256"]:
            raise ValueError(f"existing {ecosystem} author input changed: {entry['source']}")
    descriptor = next(entry for entry in inputs if entry["path"] == "dever-runtime.json")
    manifest = json.loads((root / descriptor["source"]).read_text(encoding="utf-8"))
    if manifest["ecosystem"] != ecosystem:
        raise ValueError("runtime ecosystem differs from the selected author input")
    if ecosystem == "pip" and (manifest["python_wheel_tags"] != settings["python_wheel_tags"] or manifest["python_markers"] != settings["python_markers"]):
        raise ValueError("Python runtime and registry descriptors disagree")
    output.mkdir(parents=True, exist_ok=False)
    try:
        pack = output / "runtime.pack"
        with pack.open("xb") as target, gzip.GzipFile(filename="", mode="wb", fileobj=target, mtime=0) as compressed, tarfile.open(fileobj=compressed, mode="w|") as archive:
            for entry in inputs:
                source = root / entry["source"]
                info = tarfile.TarInfo(relative_path(entry["path"]))
                info.size = source.stat().st_size
                info.mode = 0o755 if entry["path"] == manifest["executable"] else 0o644
                with source.open("rb") as payload:
                    archive.addfile(info, payload)
                if digest(source) != entry["sha256"]:
                    raise ValueError(f"{ecosystem} author input changed during archive creation: {entry['source']}")
        runtime = {
            "pack": {"name": settings["name"], "version": settings["version"], "sha256": digest(pack)},
            "target": manifest["target"],
            "python_markers": None,
            "python_wheel_tags": [],
        }
        metadata = ("python_markers", "python_wheel_tags") if ecosystem == "pip" else ("npm_libc",)
        runtime.update({field: settings[field] for field in metadata})
        (output / "config").mkdir()
        (output / "config/setting.json").write_text(json.dumps({"runtime": runtime}, indent=2) + "\n", encoding="utf-8")
    except BaseException:
        shutil.rmtree(output)
        raise
    print(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--refresh", action="store_true", help="refresh descriptors of existing verified inputs without extracting a second copy")
    action.add_argument("--python-worker", type=Path, help="write a fresh explicit Python Worker fixture pack from already prepared inputs")
    action.add_argument("--npm-worker", type=Path, help="write a fresh explicit Node Worker fixture pack from already prepared inputs")
    arguments = parser.parse_args()
    root = arguments.output.absolute()
    for ecosystem, destination in (("pip", arguments.python_worker), ("npm", arguments.npm_worker)):
        if destination is not None:
            worker_runtime(root, destination.absolute(), ecosystem)
            return
    if arguments.refresh:
        setting_path = root / "config/setting.json"
        settings = json.loads(setting_path.read_text(encoding="utf-8"))
        for runtime in settings["runtimes"].values():
            for entry in runtime["files"]:
                if digest(root / relative_path(entry["source"])) != entry["sha256"]:
                    raise ValueError(f"existing author input changed: {entry['source']}")
        settings["runtimes"] = {ecosystem: runtime_settings(root, ecosystem) for ecosystem in SOURCES}
        setting_path.write_text(json.dumps(settings, indent=2) + "\n", encoding="utf-8")
        print(root)
        return
    root.mkdir(parents=True, exist_ok=False)
    try:
        provenance = {}
        runtimes = {}
        for ecosystem in SOURCES:
            destination = root / ecosystem
            destination.mkdir()
            provenance[ecosystem] = materialize(ecosystem, destination)
            runtimes[ecosystem] = runtime_settings(root, ecosystem)
        (root / "config").mkdir()
        (root / "config/setting.json").write_text(json.dumps({"runtimes": runtimes}, indent=2) + "\n", encoding="utf-8")
        (root / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    except BaseException:
        shutil.rmtree(root)
        raise
    print(root)


if __name__ == "__main__":
    main()

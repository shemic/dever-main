"""Fixed PEP 517 frontend, executed only inside the managed build sandbox."""

import importlib
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tomllib


def build_system(source):
    project = source / "pyproject.toml"
    config = tomllib.loads(project.read_text("utf-8")) if project.exists() else {}
    system = config.get("build-system")
    if system is None:
        system = {"requires": ["setuptools>=40.8.0"], "build-backend": "setuptools.build_meta:__legacy__"}
    if not isinstance(system, dict) or not isinstance(system.get("requires"), list):
        raise ValueError("build-system.requires must be an array")
    requires = system["requires"]
    if len(requires) > 1024 or any(not isinstance(value, str) for value in requires):
        raise ValueError("invalid static build requirements")
    backend = system.get("build-backend", "setuptools.build_meta:__legacy__")
    identifier = r"[A-Za-z_][A-Za-z_0-9]*(?:\.[A-Za-z_][A-Za-z_0-9]*)*"
    if not isinstance(backend, str) or not re.fullmatch(identifier + "(?::" + identifier + ")?", backend):
        raise ValueError("invalid build-backend entry point")
    paths = system.get("backend-path", [])
    if not isinstance(paths, list) or len(paths) > 64:
        raise ValueError("invalid backend-path")
    for name in paths:
        if not isinstance(name, str) or not name or Path(name).is_absolute() or "\\" in name:
            raise ValueError("backend-path must be source-relative")
        resolved = (source / name).resolve()
        if not resolved.is_relative_to(source.resolve()) or not resolved.is_dir():
            raise ValueError("backend-path escapes the source tree")
    return {"requires": requires, "backend": backend, "backend_path": paths}


def backend_object(source, system):
    paths = [(source / path).resolve() for path in system["backend_path"]]
    sys.path[:0] = [str(path) for path in paths]
    module_name, _, object_name = system["backend"].partition(":")
    module = importlib.import_module(module_name)
    if paths:
        origin = Path(module.__file__).resolve()
        if not any(origin.is_relative_to(path) for path in paths):
            raise ValueError("in-tree backend was loaded outside backend-path")
    backend = module
    for part in object_name.split(".") if object_name else []:
        backend = getattr(backend, part)
    return backend


def leaf(value, suffix):
    if not isinstance(value, str) or Path(value).name != value or not value.endswith(suffix):
        raise ValueError("build hook returned an invalid output name")
    return value


def execute(request, source, output):
    system = build_system(source)
    if request["phase"] == "inspect":
        return system
    if system != request["system"]:
        raise ValueError("source build configuration changed between hooks")
    backend = backend_object(source, system)
    settings = request["config_settings"]
    if request["phase"] == "requires":
        hook = getattr(backend, "get_requires_for_build_wheel", None)
        requires = hook(settings) if hook else []
        if not isinstance(requires, list) or len(requires) > 1024 or any(not isinstance(value, str) for value in requires):
            raise ValueError("invalid dynamic build requirements")
        return {"requires": requires}
    if request["phase"] == "metadata":
        metadata = output / "metadata"
        metadata.mkdir()
        hook = getattr(backend, "prepare_metadata_for_build_wheel", None)
        prepared = metadata / leaf(hook(str(metadata), settings), ".dist-info") if hook else None
        if prepared is not None and (not prepared.is_dir() or prepared.is_symlink()):
            raise ValueError("metadata hook did not create its declared directory")
        return {"metadata": str(prepared.relative_to(output)) if prepared else None}
    prepared = None
    if request.get("metadata") is not None:
        shutil.copytree("/worker/prepared", output / "prepared")
        prepared = output / "prepared" / request["metadata"]
        if not prepared.resolve().is_relative_to((output / "prepared").resolve()):
            raise ValueError("prepared metadata path escapes its snapshot")
    wheels = output / "wheels"
    wheels.mkdir()
    name = leaf(backend.build_wheel(str(wheels), settings, str(prepared) if prepared else None), ".whl")
    return {"filename": name}


def main():
    request = json.loads(Path("/worker/request.json").read_text("utf-8"))
    work = Path("/data/build")
    source = work / "source"
    shutil.copytree("/worker/source", source)
    output = work / "output"
    output.mkdir()
    (work / "tmp").mkdir()
    # These values are part of the fixed frontend, never project/host settings.
    os.environ.update({
        "PATH": "/worker/runtime/bin:/usr/bin",
        "CC": "/usr/bin/cc", "CXX": "/usr/bin/c++",
        "AR": "/usr/bin/ar", "RANLIB": "/usr/bin/ranlib",
        "LDSHARED": "/usr/bin/cc -shared",
        "CFLAGS": "-fPIC -ffile-prefix-map=/data/build/source=.",
        "CPPFLAGS": "-I/worker/runtime/include/python" + request["python_version"],
        "TMPDIR": "/data/build/tmp", "SOURCE_DATE_EPOCH": "0",
    })
    os.chdir(source)
    result = execute(request, source, output)
    (output / "result.json").write_text(json.dumps(result, sort_keys=True), "utf-8")


if __name__ == "__main__":
    main()

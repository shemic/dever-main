"""Explicit Linux author SDK leaves for offline Python/npm build acceptance.

This prepares test inputs, not a trusted public distribution. System files are
declared here and hashed; fixed Python/Node headers and npm come from the same
verified upstream archives used by prepare.py. No PATH lookup or downloads.
"""

import argparse
import json
import os
from pathlib import Path
import re
import shutil

from prepare import SOURCES, digest, materialize


TARGET = "linux-x86_64"
MULTIARCH = "x86_64-linux-gnu"
GCC_VERSION = "13"
NPM_NODE_GYP_SHIM = "npm/frontend/node_modules/npm/node_modules/@npmcli/run-script/lib/node-gyp-bin/node-gyp"
TOOLS = {
    "sh": "dash", "cc": "x86_64-linux-gnu-gcc-13", "gcc": "x86_64-linux-gnu-gcc-13",
    "c++": "x86_64-linux-gnu-g++-13", "g++": "x86_64-linux-gnu-g++-13",
    "as": "x86_64-linux-gnu-as", "ld": "x86_64-linux-gnu-ld.bfd",
    "ar": "x86_64-linux-gnu-ar", "ranlib": "x86_64-linux-gnu-ranlib",
    "nm": "x86_64-linux-gnu-nm", "strip": "x86_64-linux-gnu-strip",
    **{name: name for name in (
        "make", "env", "basename", "dirname", "cat", "cp", "mv", "rm", "mkdir",
        "rmdir", "ln", "touch", "chmod", "pwd", "printf", "test", "true", "false",
        "sed", "grep", "find", "sort", "tr", "cut", "head", "tail", "wc", "uname",
    )},
}
LIBRARIES = (
    "libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0", "librt.so.1",
    "libgcc_s.so.1", "libstdc++.so.6", "libisl.so.23", "libmpc.so.3",
    "libmpfr.so.6", "libgmp.so.10", "libz.so.1", "libzstd.so.1",
    "libbfd-2.42-system.so", "libctf.so.0", "libjansson.so.4",
    "libsframe.so.1", "libpcre2-8.so.0", "libselinux.so.1", "libacl.so.1",
    "libattr.so.1", "libtinfo.so.6", "libcrypto.so.3", "libssl.so.3",
    "libc.so", "libc_nonshared.a", "libm.so", "libm-2.39.a", "libmvec.so.1",
    "libpthread.a", "libdl.a", "librt.a", "libutil.a",
    "crt1.o", "Scrt1.o", "crti.o", "crtn.o",
)


def signed_library_paths(content):
    # 仅迁移以 /lib 开头的绝对 token，不能再次改写已有的 /usr/lib。
    return re.sub(r"(?<!\S)/lib/" + re.escape(MULTIARCH) + "/",
                  f"/usr/lib/{MULTIARCH}/", content)


def sdk_leaf(source, destination, changes):
    original = source
    source = source.resolve(strict=True)
    if not source.is_file() or not source.is_relative_to("/usr"):
        raise ValueError(f"SDK leaf must resolve to an explicit /usr file: {original}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    if original.name in ("libc.so", "libm.so", "libgcc_s.so"):
        content = source.read_text()
        rewritten = signed_library_paths(content)
        if rewritten != content:
            destination.write_text(rewritten)
            changes.append({"source": str(original), "source_sha256": digest(source),
                            "transform": "linker script uses signed /usr/lib multiarch leaves"})
            return
    # These are read-only author inputs. Never chmod a hardlink to a host file.
    os.link(source, destination)


def sdk_tree(source, destination, changes):
    for current, directories, names in os.walk(source, followlinks=False):
        current = Path(current)
        if any((current / name).is_symlink() for name in directories):
            raise ValueError(f"unexpected directory alias in explicit SDK input: {current}")
        for name in sorted(names):
            sdk_leaf(current / name, destination / (current / name).relative_to(source), changes)


def leaves(root, sources):
    entries = []
    executables = []
    for source, prefix in sources:
        for path in sorted(source.rglob("*")):
            if not path.is_file():
                continue
            name = (Path(prefix) / path.relative_to(source)).as_posix()
            entries.append({"source": path.relative_to(root).as_posix(),
                            "path": name, "sha256": digest(path)})
            if path.stat().st_mode & 0o111:
                executables.append(name)
    if len(entries) > 65_536 or sum((root / entry["source"]).stat().st_size for entry in entries) > 512 * 1024 * 1024:
        raise ValueError("author SDK exceeds the build-only pack budget")
    return {"files": entries, "executables": executables}


def prepare(output):
    output.mkdir(parents=True, exist_ok=False)
    changes = []
    try:
        rootfs = output / "rootfs"
        for name, source in TOOLS.items():
            sdk_leaf(Path("/usr/bin") / source, rootfs / "usr/bin" / name, changes)
        for source in (Path("/usr/include"),
                       Path(f"/usr/lib/gcc/{MULTIARCH}/{GCC_VERSION}"),
                       Path(f"/usr/libexec/gcc/{MULTIARCH}/{GCC_VERSION}")):
            sdk_tree(source, rootfs / source.relative_to("/"), changes)
        for name in LIBRARIES:
            source = Path("/usr/lib") / MULTIARCH / name
            sdk_leaf(source, rootfs / source.relative_to("/"), changes)
        python = output / "python"
        node = output / "node"
        materialize("pip", python, selector=lambda name: name.startswith("include/"))
        materialize("npm", node, selector=lambda name: name.startswith(("include/node/", "lib/node_modules/npm/")))
        npm = leaves(output, ((rootfs, "rootfs"), (node / "include/node", "npm/include/node"),
                              (node / "lib/node_modules/npm", "npm/frontend/node_modules/npm")))
        shim = next(entry for entry in npm["files"] if entry["path"] == NPM_NODE_GYP_SHIM)
        if not (output / shim["source"]).read_bytes().startswith(b"#!/usr/bin/env sh\n"):
            raise ValueError("pinned npm node-gyp shim changed its interpreter")
        # run-script executes this upstream shell entry; the remaining frontend
        # JavaScript is loaded by the exact packaged Node interpreter.
        npm["executables"] = sorted(set(npm["executables"]) | {NPM_NODE_GYP_SHIM})
        manifest = {
            "format": "dever-build-author-leaves-v1", "target": TARGET,
            "provenance": {"kind": "private Ubuntu GNU author SDK",
                           "gcc": GCC_VERSION, "system_linker_script_changes": changes,
                           "upstream_archives": {name: {"path": str(value[0]), "sha256": value[1]}
                                                 for name, value in SOURCES.items() if name != "go"}},
            "pip": leaves(output, ((rootfs, "rootfs"), (python / "include", "runtime/include"))),
            "npm": npm,
        }
        (output / "inputs.json").write_text(json.dumps(manifest, indent=2) + "\n")
    except BaseException:
        shutil.rmtree(output)
        raise
    print(output / "inputs.json")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    prepare(parser.parse_args().output.absolute())

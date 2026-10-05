#!/usr/bin/env python3
"""Actual-asset update transaction in an owned image; never publishes the fixture version."""

import argparse
import hashlib
import json
import os
from pathlib import Path

from verify_prepared_release import InstalledDaemon, execute, install_assets, minimal_os, response


def prepare_update(image, manifest, key, helper):
    # Reuse real payloads under a newly signed, acceptance-only catalog identity.
    # This tests the update owner without claiming a second public release exists.
    next_manifest = dict(manifest)
    major, minor, patch = map(int, manifest["version"].split("."))
    version = f"{major}.{minor}.{patch + 1}"
    next_manifest["version"] = version
    machine = image / "opt/dever"
    package = machine / "cache/downloads" / version
    package.mkdir()
    installed = machine / "versions" / manifest["version"]
    for artifact in manifest["artifacts"]:
        destination = package / artifact["path"]
        destination.parent.mkdir(parents=True, exist_ok=True)
        os.link(installed / artifact["path"], destination)
    catalog = package / "manifest.json"
    catalog.write_text(json.dumps(next_manifest, separators=(",", ":")))
    execute([helper, "--sign-catalog", catalog, key])
    (machine / "cache/downloads/latest").write_text(version)
    return version, hashlib.sha256(catalog.read_bytes()).hexdigest()


def accept(args):
    manifest = json.loads((args.release / "manifest.json").read_bytes())
    result = {"version": manifest["version"], "checks": [],
              "transport": "local actual assets; acceptance-only next-version catalog",
              "online_update": "not_run"}

    def passed(name):
        result["checks"].append(name)
        args.report.write_text(json.dumps(result, indent=2) + "\n")
        print(f"PASS {name}", flush=True)

    try:
        args.image.mkdir()
        minimal_os(args.image)
        install_assets(args.assets, args.image, manifest)
        machine = args.image / "opt/dever"
        extension = f"runtime-pip-{manifest['platform']}"
        execute([args.extension_helper, machine, args.assets, manifest["version"], extension])
        records = args.report.parent / f"{args.report.stem}-process"
        with InstalledDaemon(args.image, records, args.init) as daemon:
            daemon.command("new", "/project")
            response(daemon.command("run", "/project", "--", "hello.greeting.greet", '{"name":"Dever"}'))
            passed("actual_release_and_selected_extension_before_update")
            version, catalog = prepare_update(args.image, manifest, args.key, args.extension_helper)
            result["fixture_version"] = version
            previous_active = (machine / "state/active-version").read_bytes()
            empty_assets = args.image / "missing-extension-assets"
            empty_assets.mkdir()
            try:
                execute([args.extension_helper, machine, empty_assets, version, "--update"])
            except RuntimeError as error:
                if "cannot open actual extension asset" not in str(error):
                    raise
            else:
                raise RuntimeError("update with a missing extension should fail")
            if (machine / "state/active-version").read_bytes() != previous_active:
                raise RuntimeError("failed update changed active version")
            if daemon.command("version") != manifest["version"]:
                raise RuntimeError("failed update changed the public launcher selection")
            response(daemon.command("run", "/project", "--", "hello.greeting.greet", '{"name":"Dever"}'))
            passed("failed_extension_update_keeps_old_active_and_application_working")
            execute([args.extension_helper, machine, args.assets, version, "--update"])
            if daemon.command("version") != version:
                raise RuntimeError("successful update did not activate the new version")
            if daemon.command("skill", "path") != f"/opt/dever/versions/{version}/skills/dever-language":
                raise RuntimeError("skill did not follow the updated core")
            extensions = machine / "cache/extensions" / version / catalog
            if {path.name for path in extensions.iterdir()} != {extension}:
                raise RuntimeError("update did not preserve exactly the installed extension selection")
            passed("successful_update_activates_core_skill_and_only_installed_extensions")
            # Only the catalog version changes in this transaction fixture.
            # The managed compiler correctly requires its real build version;
            # execute it again only after selecting the actual release.
            daemon.command("use", manifest["version"])
            response(daemon.command("run", "/project", "--", "hello.greeting.greet", '{"name":"Dever"}'))
            passed("actual_release_remains_usable_after_update_and_explicit_switch_back")
        result["status"] = "passed"
    except BaseException as error:
        result["status"] = "failed"
        result["error"] = str(error)
        raise
    finally:
        args.report.write_text(json.dumps(result, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("release", "assets", "image", "report", "init", "extension-helper", "key"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    args = parser.parse_args()
    if os.geteuid() != 0:
        parser.error("requires root for an owned image; never installs host services")
    for name, value in vars(args).items():
        setattr(args, name, value.absolute())
    args.report.parent.mkdir(parents=True, exist_ok=True)
    accept(args)


if __name__ == "__main__":
    main()

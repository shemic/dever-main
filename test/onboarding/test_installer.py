"""Offline install/asset contracts; bootstrap is mocked, never installed globally."""

import argparse
import contextlib
import gzip
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


installer = load_module("dever_installer", ROOT / "skills/scripts/install.py")
assets = load_module("dever_release_assets", ROOT / "sdk/release-assets.py")


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="dever-installer-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.release = self.directory("release")
        self.scratch = self.directory("scratch")
        self.platform = installer.platform_name()
        self.payloads = {
            "dever-core": b"fixture core, never executed\n",
            "bootstrap/dever": b"fixture launcher, never executed\n",
            "skills/dever-language/SKILL.md": b"fixture skill\n",
        }
        self.catalog = {}
        for name, content in self.payloads.items():
            path = self.release / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content)
            self.catalog[name] = {
                "path": name, "bytes": len(content),
                "sha256": hashlib.sha256(content).hexdigest(),
            }
        manifest = {
            "format": "dever-release-v1", "platform": self.platform,
            "version": "1.2.3", "artifacts": list(self.catalog.values()),
        }
        (self.release / "manifest.json").write_text(json.dumps(manifest))
        self.private = self.root / "test-key.pem"
        self.openssl("genpkey", "-algorithm", "ED25519", "-out", str(self.private))
        public_der = self.openssl("pkey", "-in", str(self.private), "-pubout", "-outform", "DER")
        self.public = self.root / "test-key.pub"
        self.public.write_text(public_der[-32:].hex())
        self.sign_manifest()

    def sign_manifest(self):
        signature = self.root / "signature.bin"
        self.openssl("pkeyutl", "-sign", "-inkey", str(self.private), "-rawin",
                     "-in", str(self.release / "manifest.json"), "-out", str(signature))
        (self.release / "manifest.sig").write_text(signature.read_bytes().hex())

    def directory(self, name):
        path = self.root / name
        path.mkdir()
        return path

    def openssl(self, *arguments):
        return subprocess.run(["/usr/bin/openssl", *arguments], env={},
                              capture_output=True, check=True, timeout=15).stdout

    def authenticate(self, key=None):
        return installer.authenticate(self.release, key or self.public, self.scratch,
                                      self.platform, "1.2.3")

    def archive(self, entries, trailer=None):
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name, content, kind in entries:
                member = tarfile.TarInfo(name)
                member.type = kind
                member.size = len(content) if kind == tarfile.REGTYPE else 0
                if kind == tarfile.SYMTYPE:
                    member.linkname = "../../outside"
                archive.addfile(member, io.BytesIO(content) if member.size else None)
        contents = stream.getvalue()
        if trailer is not None:
            contents += trailer
        return gzip.compress(contents)

    def extract(self, compressed, destination):
        with mock.patch.object(installer, "download", return_value=io.BytesIO(compressed)):
            installer.payload(destination, None, "1.2.3", self.platform, self.catalog)

    def test_signature_accepts_independent_key_and_rejects_wrong_key_or_changed_manifest(self):
        version, catalog, key = self.authenticate()
        self.assertEqual(version, "1.2.3")
        self.assertEqual(catalog, self.catalog)
        self.assertEqual(len(key), 32)
        wrong = self.root / "wrong.pub"
        wrong.write_text(bytes(32).hex())
        with self.assertRaisesRegex(ValueError, "签名校验失败"):
            self.authenticate(wrong)
        with (self.release / "manifest.json").open("a") as stream:
            stream.write(" ")
        with self.assertRaisesRegex(ValueError, "签名校验失败"):
            self.authenticate()

    def test_asset_author_round_trip_is_deterministic_and_contains_only_signed_payload(self):
        first = self.root / "first"
        second = self.root / "second"
        (self.release / "unlisted").write_text("not published")
        names = assets.create(self.release, first)
        self.assertEqual(names, assets.create(self.release, second))
        self.assertEqual(len(names), 3)
        for name in names:
            self.assertEqual((first / name).read_bytes(), (second / name).read_bytes())
        extracted = self.directory("extracted")
        self.extract((first / f"dever-{self.platform}.tar.gz").read_bytes(), extracted)
        actual = {str(path.relative_to(extracted)): path.read_bytes()
                  for path in extracted.rglob("*") if path.is_file()}
        self.assertEqual(actual, self.payloads)
        self.assertEqual((extracted / "bootstrap/dever").stat().st_mode & 0o777, 0o755)
        with self.assertRaisesRegex(ValueError, "already exists"):
            assets.create(self.release, first)

    def test_signed_release_without_matching_skill_is_rejected(self):
        path = self.release / "manifest.json"
        manifest = json.loads(path.read_text())
        manifest["artifacts"] = [entry for entry in manifest["artifacts"]
                                 if not entry["path"].startswith("skills/")]
        path.write_text(json.dumps(manifest))
        self.sign_manifest()
        with self.assertRaisesRegex(ValueError, "配套 skill"):
            self.authenticate()

    def test_publish_preserves_concurrent_targets_and_owned_staging_on_failure(self):
        staging = self.directory("staging")
        (staging / "asset").write_bytes(b"owned release bytes")
        empty = self.directory("empty")
        populated = self.directory("populated")
        (populated / "existing").write_bytes(b"existing user bytes")
        file = self.root / "file"
        file.write_bytes(b"existing file bytes")
        symlink = self.root / "symlink"
        symlink.symlink_to(self.root / "missing")
        for destination in (empty, populated, file, symlink):
            with self.subTest(destination=destination.name), self.assertRaises(OSError):
                assets.publish(staging, destination)
            self.assertEqual((staging / "asset").read_bytes(), b"owned release bytes")
        self.assertEqual(list(empty.iterdir()), [])
        self.assertEqual((populated / "existing").read_bytes(), b"existing user bytes")
        self.assertEqual(file.read_bytes(), b"existing file bytes")
        self.assertEqual(symlink.readlink(), self.root / "missing")
        destination = self.root / "published"
        assets.publish(staging, destination)
        self.assertFalse(staging.exists())
        self.assertEqual((destination / "asset").read_bytes(), b"owned release bytes")

    def test_create_cleans_only_its_staging_when_target_appears_during_preparation(self):
        actual_publish = assets.publish
        for populated in (False, True):
            output = self.root / ("race-populated" if populated else "race-empty")

            def publish(prepared, destination):
                destination.mkdir()
                if populated:
                    (destination / "user-file").write_bytes(b"concurrent user bytes")
                actual_publish(prepared, destination)

            with mock.patch.object(assets, "publish", side_effect=publish), \
                    self.assertRaises(OSError):
                assets.create(self.release, output)
            self.assertTrue(output.is_dir())
            if populated:
                self.assertEqual((output / "user-file").read_bytes(), b"concurrent user bytes")
            else:
                self.assertEqual(list(output.iterdir()), [])
            self.assertEqual(list(self.root.glob(".dever-release-assets-*")), [])

    def test_archive_rejects_unsigned_duplicate_symlink_wrong_size_missing_and_bad_tail(self):
        normal = [(name, content, tarfile.REGTYPE) for name, content in self.payloads.items()]
        cases = {
            "unsigned": normal + [("outside", b"extra", tarfile.REGTYPE)],
            "duplicate": normal + [normal[0]],
            "symlink": [(normal[0][0], b"", tarfile.SYMTYPE)] + normal[1:],
            "size": [(normal[0][0], b"short", tarfile.REGTYPE)] + normal[1:],
            "missing": normal[:-1],
        }
        for name, entries in cases.items():
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.extract(self.archive(entries), self.directory(name))
        with self.assertRaisesRegex(ValueError, "尾部"):
            self.extract(self.archive(normal, b"hidden"), self.directory("tail"))
        corrupted = [(normal[0][0], b"x" * len(normal[0][1]), tarfile.REGTYPE)] + normal[1:]
        with self.assertRaisesRegex(ValueError, "摘要"):
            self.extract(self.archive(corrupted), self.directory("digest"))

    def test_local_payload_rejects_symlink_and_source_directory(self):
        copied = self.directory("copied")
        installer.payload(copied, self.release, "1.2.3", self.platform, self.catalog)
        self.assertEqual((copied / "dever-core").read_bytes(), self.payloads["dever-core"])
        core = self.release / "dever-core"
        core.unlink()
        core.symlink_to(self.public)
        with self.assertRaisesRegex(ValueError, "符号链接"):
            installer.payload(self.directory("symlink"), self.release, "1.2.3", self.platform, self.catalog)
        with self.assertRaisesRegex(ValueError, "symlinks"):
            assets.create(self.release, self.root / "invalid-assets")
        core.unlink()
        core.mkdir()
        with self.assertRaisesRegex(ValueError, "类型"):
            installer.payload(self.directory("directory"), self.release, "1.2.3", self.platform, self.catalog)

    def test_image_install_authenticates_before_mock_bootstrap_and_cleans_owned_staging(self):
        image = self.directory("image")
        arguments = argparse.Namespace(version="1.2.3", system_root=image, release=self.release,
                                       trusted_key=self.public, no_start=False)
        actual_run = subprocess.run
        calls = []

        def run(command, **kwargs):
            if command[0] == "/usr/bin/openssl":
                return actual_run(command, **kwargs)
            self.assertEqual(command[1], "--dever-install")
            scratch = Path(command[2])
            setting = json.loads((scratch / "config/setting.json").read_text())
            self.assertEqual(set(setting), {"release", "system_root", "trusted_key", "activate_service"})
            self.assertEqual(setting["system_root"], str(image))
            self.assertFalse(setting["activate_service"])
            self.assertEqual(kwargs["env"], {})
            self.assertFalse(Path(setting["trusted_key"]).is_relative_to(Path(setting["release"])))
            calls.append(command)
            return subprocess.CompletedProcess(command, 0)

        with mock.patch.object(installer.os, "geteuid", return_value=0), \
                mock.patch.object(installer.subprocess, "run", side_effect=run), \
                contextlib.redirect_stdout(io.StringIO()):
            installer.install(arguments)
            self.assertEqual(len(calls), 1)
            self.assertEqual(list(image.iterdir()), [])
            (self.release / "dever-core").write_bytes(b"corrupt")
            with self.assertRaises(ValueError):
                installer.install(arguments)
        self.assertEqual(len(calls), 1)
        self.assertEqual(list(image.iterdir()), [])


if __name__ == "__main__":
    unittest.main()

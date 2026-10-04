"""只执行 peer 的配置检查入口，不创建网络或连接数据库。"""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from settings import configured_peer, stage_peer


class PeerSettingsTests(unittest.TestCase):
    def check(self, peer, document):
        with tempfile.TemporaryDirectory(prefix="dever-peer-config-test-") as temporary:
            root = Path(temporary)
            executable = stage_peer(configured_peer(peer), root, {})
            setting = root / "config/setting.json"
            if document is None:
                setting.unlink()
            else:
                setting.write_text(json.dumps(document))
            # 不在 executable 目录启动，验证读取位置不受工作目录影响。
            return subprocess.run([executable, "check-config"], cwd=root.parent,
                                  capture_output=True, text=True, timeout=5)

    def test_both_peers_use_the_external_integer_setting(self):
        for peer in ("network_peer", "live_peer"):
            for workers in (1, 4, 64):
                with self.subTest(peer=peer, workers=workers):
                    output = self.check(peer, {"benchmark": {"workers": workers}})
                    self.assertEqual(output.returncode, 0, output.stderr)
                    self.assertEqual(json.loads(output.stdout), {"workers": workers})

    def test_both_peers_reject_missing_malformed_and_out_of_range_configuration(self):
        invalid = [None, {}, [], {"benchmark": []}, {"benchmark": {"unknown": 1}}]
        invalid.extend({"benchmark": {"workers": value}} for value in (0, 65, -1, 1.5, True, "4", None))
        for peer in ("network_peer", "live_peer"):
            for document in invalid:
                with self.subTest(peer=peer, document=document):
                    output = self.check(peer, document)
                    self.assertNotEqual(output.returncode, 0)
                    self.assertEqual(output.stdout, "")
                    self.assertTrue(output.stderr.strip())


if __name__ == "__main__":
    unittest.main()

"""作者工具的 GNU 链接脚本迁移合同；不读取或修改宿主工具。"""

import unittest

from build_inputs import MULTIARCH, signed_library_paths


class LinkerScriptTests(unittest.TestCase):
    def test_rewrites_only_root_library_tokens(self):
        original = (f"GROUP ( /lib/{MULTIARCH}/libc.so.6 "
                    f"/usr/lib/{MULTIARCH}/libc_nonshared.a "
                    "AS_NEEDED ( /lib64/ld-linux-x86-64.so.2 ) )")
        expected = original.replace(f"( /lib/{MULTIARCH}/", f"( /usr/lib/{MULTIARCH}/")
        self.assertEqual(signed_library_paths(original), expected)
        self.assertEqual(signed_library_paths(expected), expected)

    def test_keeps_relative_and_other_target_paths(self):
        original = "GROUP ( libgcc_s.so.1 -lgcc /lib/other-target/libc.so )"
        self.assertEqual(signed_library_paths(original), original)


if __name__ == "__main__":
    unittest.main()

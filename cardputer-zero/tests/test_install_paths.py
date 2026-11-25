"""Pure packaging policy checks; no device or third-party dependencies."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("install_paths", Path(__file__).parents[1] / "scripts/check_install_paths.py")
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class InstallPaths(unittest.TestCase):
    def test_identity_and_normalized_prefix(self):
        self.assertTrue(policy.allowed("usr/bin/colibri", "colibri"))
        self.assertFalse(policy.allowed("usr/bin/colibri", "colibri-cardputer"))
        self.assertTrue(policy.allowed("usr/share/font_preview/fonts/main.ttf", "fontpreview"))
        self.assertFalse(policy.allowed("usr/share/preview/main.ttf", "fontpreview"))

    def test_standard_positions_and_exceptions(self):
        for path in ["usr/lib/colibri/worker", "usr/share/colibri/pinyin/words.tsv", "opt/colibri/bin/run",
                     "etc/colibri.conf", "etc/colibri/config", "var/lib/colibri/state", "etc/default/colibri",
                     "etc/logrotate.d/colibri", "usr/share/lintian/overrides/colibri",
                     "usr/share/bash-completion/completions/colibri", "usr/lib/tmpfiles.d/colibri.conf",
                     "usr/share/man/man1/colibri.1.gz", "usr/lib/udev/rules.d/90-colibri.rules",
                     "lib/systemd/system/colibri.service", "usr/lib/systemd/system/colibri.service",
                     "usr/share/APPLaunch/applications/colibri.desktop", "usr/share/doc/colibri/copyright"]:
            self.assertTrue(policy.allowed(path, "colibri"), path)

    def test_forbidden_positions(self):
        for path in ["usr/local/bin/colibri", "bin/colibri", "sbin/colibri", "lib/colibri/worker",
                     "lib64/colibri/worker", "usr/bin/colibri/worker", "usr/bin/unrelated",
                     "usr/share/colibri.png", "usr/lib/colibri.so", "usr/share/unrelated/font.ttf",
                     "usr/lib/tmpfiles.d/colibri.txt", "usr/lib/udev/rules.d/90-unrelated.rules"]:
            self.assertFalse(policy.allowed(path, "colibri"), path)


if __name__ == "__main__":
    unittest.main()

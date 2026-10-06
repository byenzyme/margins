"""E2E and smoke scripts run on the macOS gate too, so they must not depend
on GNU-only tools.

`stat -c` is GNU coreutils; BSD/macOS `stat` rejects it, so a probe written as
`stat -c … || echo absent` silently reads as "absent" on a Mac and the check
it guards becomes vacuous. `sha256sum` and `readlink -f` are missing or
different there too. Use python3, or pair the GNU form with its BSD fallback
(`stat -c %a f 2>/dev/null || stat -f %Lp f`).
"""
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = sorted(
    {
        *ROOT.glob("scripts/e2e-*.sh"),
        ROOT / "scripts" / "core-product-smoke.sh",
        ROOT / "scripts" / "smoke-official-cli.sh",
        *ROOT.glob("tests/*.sh"),
        ROOT / "install.sh",
    }
)


def code_lines(path: Path):
    for number, line in enumerate(path.read_text().splitlines(), 1):
        stripped = line.lstrip()
        if stripped.startswith("#"):
            continue
        yield number, line


class PortableScriptsTest(unittest.TestCase):
    def test_gnu_stat_has_a_bsd_fallback(self):
        offenders = [
            f"{path.relative_to(ROOT)}:{number}"
            for path in SCRIPTS
            if path.is_file()
            for number, line in code_lines(path)
            if re.search(r"\bstat\s+-c\b", line) and not re.search(r"\bstat\s+-f\b", line)
        ]
        self.assertEqual(offenders, [], "GNU `stat -c` without a BSD `stat -f` fallback")

    def test_no_gnu_only_hashing_or_readlink(self):
        offenders = [
            f"{path.relative_to(ROOT)}:{number}"
            for path in SCRIPTS
            if path.is_file()
            for number, line in code_lines(path)
            if re.search(r"(^|[\s;|&(`$])(sha256sum|md5sum)\b", line)
            or re.search(r"\breadlink\s+-f\b", line)
        ]
        self.assertEqual(offenders, [], "GNU-only sha256sum/md5sum/readlink -f")


if __name__ == "__main__":
    unittest.main()

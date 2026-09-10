"""Guard the integrations design docs against silent truncation.

A PR once shrank the rationale from 399 to 189 lines and the contract from
318 to 129 with every code gate green. These floors and required headings make
that impossible to merge unnoticed. Raise a floor when a doc legitimately
grows; never lower one without a deliberate, reviewed reason.
"""
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = {
    "desktop/INTEGRATIONS_DESIGN_RATIONALE.md": {
        "min_lines": 400,
        "headings": [
            "## 1. What this layer is for",
            "## 2. Why a SQLite ledger",
            "## 3. Schema rationale",
            "## 4. Identity doctrine",
            "## 5. Evidence, not filters",
            "## 6. Native Google as transport",
            "## 7. Known tensions",
            "## 8. What real-machine validation established",
            "## 9. Open questions",
        ],
    },
    "desktop/INTEGRATIONS_CONNECTOR_CONTRACT.md": {
        "min_lines": 300,
        "headings": ["## Doctrine", "## Normalized record", "## Connector trait"],
    },
    "desktop/INTEGRATIONS_RECALL_SQLITE_SOURCE.md": {
        "min_lines": 120,
        "headings": ["## Minimal contract", "## Incremental reconciliation"],
    },
}


class DesignDocIntegrity(unittest.TestCase):
    def test_docs_keep_their_size_and_sections(self):
        for rel, spec in DOCS.items():
            path = ROOT / rel
            with self.subTest(doc=rel):
                self.assertTrue(path.exists(), f"{rel} missing")
                text = path.read_text(encoding="utf-8")
                lines = text.count("\n")
                self.assertGreaterEqual(
                    lines, spec["min_lines"],
                    f"{rel} has {lines} lines, below floor {spec['min_lines']} — was it truncated?",
                )
                for heading in spec["headings"]:
                    self.assertTrue(
                        any(line.startswith(heading) for line in text.splitlines()),
                        f"{rel} lost required section starting with {heading!r}",
                    )


if __name__ == "__main__":
    unittest.main()

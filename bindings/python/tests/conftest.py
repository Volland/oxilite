"""Allow-listed divergences from pyoxigraph.

Every `py:<file>::<class>::<test>` entry of `testsuite/allowlist.toml` marks that test
`xfail(strict=True)`: an unlisted failure fails the run, and so does a listed test that passes
(a stale entry), which is the rule the Node checker enforces for `js:` entries.
"""

from __future__ import annotations

import re
from pathlib import Path
from typing import Dict, List

import pytest

ALLOWLIST = Path(__file__).resolve().parents[3] / "testsuite" / "allowlist.toml"


def _divergences() -> Dict[str, str]:
    if not ALLOWLIST.exists():
        return {}
    out = {}
    for block in ALLOWLIST.read_text(encoding="utf-8").split("[[divergence]]"):
        ident = re.search(r'^id = "py:(.*)"$', block, re.M)
        reason = re.search(r'^reason = "(.*)"$', block, re.M)
        if ident:
            out[ident.group(1)] = reason.group(1) if reason else "allow-listed divergence"
    return out


def _test_id(item: pytest.Item) -> str:
    file, *rest = item.nodeid.split("::")
    return "::".join([Path(file).name, *rest])


# @lat: [[tests#Python#Ported pyoxigraph tests]]
def pytest_collection_modifyitems(config: pytest.Config, items: List[pytest.Item]) -> None:
    divergences = _divergences()
    for item in items:
        reason = divergences.get(_test_id(item))
        if reason is not None:
            item.add_marker(pytest.mark.xfail(strict=True, reason=reason))

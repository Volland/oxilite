"""The Python examples and the package README run as written.

Each script in `examples/python/` (and the tutorial's `examples/python-tour/tour.py`) asserts its own
output, so running it is the test. The README's code blocks run in order, sharing one namespace.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from typing import Any, Dict, List

import pytest

REPO = Path(__file__).resolve().parents[3]
PACKAGE = Path(__file__).resolve().parents[1]
EXAMPLES: List[Path] = sorted((REPO / "examples" / "python").glob("*.py")) + [
    REPO / "examples" / "python-tour" / "tour.py"
]


# @lat: [[tests#Python#Examples and README run as written]]
@pytest.mark.parametrize("script", EXAMPLES, ids=lambda p: p.name)
def test_example_runs(script: Path, tmp_path: Path) -> None:
    if not script.exists():
        pytest.skip("examples are not part of the source distribution")
    done = subprocess.run([sys.executable, str(script)], cwd=tmp_path, capture_output=True, text=True, timeout=120)
    assert done.returncode == 0, done.stdout + done.stderr


def test_readme_blocks_run(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    blocks = re.findall(r"```python\n(.*?)```", (PACKAGE / "README.md").read_text(encoding="utf-8"), re.S)
    assert blocks
    monkeypatch.chdir(tmp_path)  # the quick start writes data.sqlite
    namespace: Dict[str, Any] = {}
    for block in blocks:
        exec(compile(block, "README.md", "exec"), namespace)

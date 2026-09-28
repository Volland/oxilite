"""Concurrency: store calls release the GIL, so threads run queries in parallel, and asyncio code can
hand each call to a thread to keep its event loop free.

    python examples/python/08_threads_and_asyncio.py
"""

import asyncio
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from oxilite import Store

path = Path(tempfile.mkdtemp()) / "numbers.sqlite"
store = Store(path)
store.update(
    "INSERT DATA { "
    + " ".join(f"<http://example.org/n{i}> <http://example.org/value> {i} ." for i in range(1000))
    + " }"
)

COUNT_ABOVE = "SELECT (COUNT(*) AS ?n) WHERE {{ ?s <http://example.org/value> ?v FILTER(?v >= {}) }}"


def count_above(threshold: int) -> int:
    return int(next(iter(store.query(COUNT_ABOVE.format(threshold))))["n"].value)


# One store shared by a thread pool.
with ThreadPoolExecutor(max_workers=4) as pool:
    counts = list(pool.map(count_above, [0, 250, 500, 750]))
print(counts)  # [1000, 750, 500, 250]
assert counts == [1000, 750, 500, 250]


# asyncio: run each blocking call in a worker thread.
async def main() -> list:
    return await asyncio.gather(*(asyncio.to_thread(count_above, t) for t in (100, 900)))


print(asyncio.run(main()))  # [900, 100]
assert asyncio.run(main()) == [900, 100]

# A read-only handle on the same file, for a reader process or thread.
reader = Store.read_only(str(path))
assert len(reader) == 1000
print("concurrency complete")

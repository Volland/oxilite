"""Versioning and time travel: an immutable change log, commits with authors, queries at past versions,
diffs, and purging data from the present and the past.

    python examples/python/06_time_travel.py
"""

import tempfile
from pathlib import Path

from oxilite import NamedNode, Store

path = Path(tempfile.mkdtemp()) / "tickets.sqlite"
store = Store(path, versioning="log")  # "off" (default), "stamped" or "log"

T1 = "<http://example.org/t1>"
STATUS = "<http://example.org/status>"
ASSIGNEE = "<http://example.org/assignee>"

with store.commit(author="ada", message="open the ticket"):
    store.update(f'INSERT DATA {{ {T1} {STATUS} "open" ; {ASSIGNEE} "grace" }}')

with store.commit(author="grace", message="start work"):
    store.update(f'DELETE DATA {{ {T1} {STATUS} "open" }} ; INSERT DATA {{ {T1} {STATUS} "in progress" }}')

with store.commit(author="grace", message="close it"):
    store.update(f'DELETE DATA {{ {T1} {STATUS} "in progress" }} ; INSERT DATA {{ {T1} {STATUS} "done" }}')

# Any query can read a past version: HEAD~n, #tick, or @timestamp.
status = f"SELECT ?s WHERE {{ {T1} {STATUS} ?s }}"
for ref in ["HEAD", "HEAD~1", "HEAD~2"]:
    print(ref, [s["s"].value for s in store.query(status, as_of=ref)])
# HEAD ['done'] / HEAD~1 ['in progress'] / HEAD~2 ['open']
assert [s["s"].value for s in store.query(status, as_of="HEAD~2")] == ["open"]

# The history, newest first.
writes = [(c.author, c.message) for c in store.history() if c.kind == "write"]
print(writes)
assert writes[:3] == [("grace", "close it"), ("grace", "start work"), ("ada", "open the ticket")]

# The net change between two versions.
for change in store.diff("HEAD~2", "HEAD"):
    print("+" if change.added else "-", change.quad)
assert sorted(c.added for c in store.diff("HEAD~2", "HEAD")) == [False, True]

# Cypher and Datalog read the past too.
old = store.cypher("MATCH (t) WHERE t.`ex:status` IS NOT NULL RETURN t.`ex:status` AS s",
                   prefixes={"ex": "http://example.org/"}, as_of="HEAD~2")
assert old.records == [{"s": "open"}]

# The history is RDF as well, in the graph <oxilite:history>.
assert store.query("ASK { GRAPH <oxilite:history> { ?s ?p ?o } }")

# Erasure: purge removes a subject from the present and from every past version.
store.purge(subject=NamedNode("http://example.org/t1"), reason="erasure request")
assert list(store.query(status)) == []
assert list(store.query(status, as_of="HEAD~3")) == []
print(store.versioning())
print("time travel complete")

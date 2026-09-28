"""Quick start: a SPARQL store in one SQLite file.

Loads Turtle, adds and removes quads, runs SELECT / ASK / CONSTRUCT, serializes results, dumps the
store and reopens it from disk.

    pip install oxilite
    python examples/python/01_quickstart.py
"""

import tempfile
from pathlib import Path

from oxilite import Literal, NamedNode, Quad, QueryResultsFormat, RdfFormat, Store, parse, serialize

EX = "http://example.org/"
PREFIXES = {"ex": EX, "foaf": "http://xmlns.com/foaf/0.1/"}
path = Path(tempfile.mkdtemp()) / "people.sqlite"

# A store is one SQLite file, created on first use. Store() with no path lives in memory.
store = Store(path)
store.load(
    """
    @prefix ex:   <http://example.org/> .
    @prefix foaf: <http://xmlns.com/foaf/0.1/> .
    ex:ada   foaf:name "Ada"   ; ex:born 1815 ; foaf:knows ex:grace .
    ex:grace foaf:name "Grace" ; ex:born 1906 .
    """,
    RdfFormat.TURTLE,
)

# Quad-level access, with the same terms as pyoxigraph.
alan = NamedNode(EX + "alan")
store.add(Quad(alan, NamedNode("http://xmlns.com/foaf/0.1/name"), Literal("Alan")))
store.add(Quad(alan, NamedNode(EX + "born"), Literal(1912)))
print(len(store))  # 7
assert len(store) == 7
assert Quad(alan, NamedNode(EX + "born"), Literal(1912)) in store

# SELECT: iterate solutions and read values by variable name.
solutions = store.query(
    "SELECT ?name ?born WHERE { ?p foaf:name ?name ; ex:born ?born } ORDER BY ?born", prefixes=PREFIXES
)
people = [(s["name"].value, int(s["born"].value)) for s in solutions]
print(people)  # [('Ada', 1815), ('Grace', 1906), ('Alan', 1912)]
assert people == [("Ada", 1815), ("Grace", 1906), ("Alan", 1912)]

# ASK returns something you can use as a bool.
assert store.query("ASK { ex:ada foaf:knows ex:grace }", prefixes=PREFIXES)

# CONSTRUCT returns triples.
triples = list(store.query("CONSTRUCT { ?b ex:knownBy ?a } WHERE { ?a foaf:knows ?b }", prefixes=PREFIXES))
print(triples[0])  # <http://example.org/grace> <http://example.org/knownBy> <http://example.org/ada>
assert triples[0].subject == NamedNode(EX + "grace")

# Results serialize to SPARQL JSON, XML, CSV or TSV.
csv = store.query("SELECT ?name WHERE { ?p foaf:name ?name } ORDER BY ?name", prefixes=PREFIXES).serialize(
    format=QueryResultsFormat.CSV
)
print(csv.decode().split())  # ['name', 'Ada', 'Alan', 'Grace']
assert csv == b"name\r\nAda\r\nAlan\r\nGrace\r\n"

# Rows as plain dicts: ready for json.dumps or pandas.DataFrame(rows).
solutions = store.query("SELECT ?name ?born WHERE { ?p foaf:name ?name ; ex:born ?born }", prefixes=PREFIXES)
rows = [{v.value: s[v].value for v in solutions.variables} for s in solutions]
assert {"name": "Ada", "born": "1815"} in rows

# SPARQL Update runs atomically.
store.update("PREFIX ex: <http://example.org/> DELETE WHERE { ?p ex:born ?y }")
assert len(store) == 4

# Dump to any RDF format, and parse or serialize without a store.
nquads = store.dump(format=RdfFormat.N_QUADS)
quads = list(parse(nquads, RdfFormat.N_QUADS))
assert len(quads) == 4
print(serialize(quads, format=RdfFormat.TURTLE).decode())
assert b'"Grace"' in serialize(quads, format=RdfFormat.TURTLE)

# The data is on disk: close by dropping the store, then reopen the file.
del store
reopened = Store(path)
print(len(reopened))  # 4
assert len(reopened) == 4
print("quickstart complete")

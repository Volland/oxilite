"""The code of the article "How to use oxilite with Python" (site/articles/oxilite-python.html),
step by step, with the outputs the article shows asserted. Run it with the package installed:

    python examples/python-tour/tour.py
"""

import asyncio
import tempfile
from datetime import datetime, timezone
from pathlib import Path

work = Path(tempfile.mkdtemp())
TEAM = work / "team.sqlite"

# --- Step 1: a knowledge graph in one file ----------------------------------------------
from oxilite import Literal, NamedNode, Quad, RdfFormat, Store

store = Store(TEAM)  # created on first use; Store() is in memory
store.load(
    """
    @prefix ex:   <http://example.org/> .
    @prefix foaf: <http://xmlns.com/foaf/0.1/> .

    ex:ada   a ex:Engineer ; foaf:name "Ada"   ; ex:age 36 ; foaf:knows ex:grace .
    ex:grace a ex:Engineer ; foaf:name "Grace" ; ex:age 45 ; foaf:knows ex:alan .
    ex:alan  a ex:Researcher ; foaf:name "Alan" ; ex:age 41 .
    """,
    RdfFormat.TURTLE,
)

EX = "http://example.org/"
store.add(Quad(NamedNode(EX + "alan"), NamedNode(EX + "age"), Literal(42)))
store.remove(Quad(NamedNode(EX + "alan"), NamedNode(EX + "age"), Literal(41)))
assert len(store) == 11

# --- Step 2: SPARQL ---------------------------------------------------------------------
PREFIXES = {"ex": EX, "foaf": "http://xmlns.com/foaf/0.1/"}

solutions = store.query(
    "SELECT ?name ?age WHERE { ?p foaf:name ?name ; ex:age ?age } ORDER BY DESC(?age)",
    prefixes=PREFIXES,
)
print([v.value for v in solutions.variables])  # ['name', 'age']
people = [(s["name"].value, int(s["age"].value)) for s in solutions]
print(people)  # [('Grace', 45), ('Alan', 42), ('Ada', 36)]
assert people == [("Grace", 45), ("Alan", 42), ("Ada", 36)]

assert store.query("ASK { ex:ada foaf:knows ex:grace }", prefixes=PREFIXES)

graph = store.query("CONSTRUCT { ?b ex:knownBy ?a } WHERE { ?a foaf:knows ?b }", prefixes=PREFIXES)
assert len(list(graph)) == 2

from oxilite import QueryResultsFormat, Variable

csv = store.query(
    "SELECT ?name WHERE { ?p foaf:name ?name } ORDER BY ?name", prefixes=PREFIXES
).serialize(format=QueryResultsFormat.CSV)
print(csv.decode())  # name\r\nAda\r\nAlan\r\nGrace\r\n
assert csv == b"name\r\nAda\r\nAlan\r\nGrace\r\n"

friends = store.query(
    "SELECT ?friend WHERE { ?p foaf:knows ?friend }",
    prefixes=PREFIXES,
    substitutions={Variable("p"): NamedNode(EX + "ada")},
)
assert [s["friend"] for s in friends] == [NamedNode(EX + "grace")]

# Rows as dicts, ready for pandas.DataFrame(rows) or a JSON API.
solutions = store.query("SELECT ?name ?age WHERE { ?p foaf:name ?name ; ex:age ?age }", prefixes=PREFIXES)
rows = [{v.value: s[v].value for v in solutions.variables} for s in solutions]
assert {"name": "Ada", "age": "36"} in rows

# --- Step 3: see the SQL ----------------------------------------------------------------
sql = store.explain("PREFIX ex: <http://example.org/> SELECT ?p WHERE { ?p a ex:Engineer ; ex:age ?a FILTER(?a > 40) }")
print(sql)
assert "SELECT" in sql and "quads" in sql

# --- Step 4: coming from pyoxigraph -----------------------------------------------------
import oxilite as pyoxigraph  # the only change most pyoxigraph code needs

legacy = pyoxigraph.Store(str(work))  # a directory: the data goes to work/oxilite.sqlite
legacy.add(pyoxigraph.Quad(pyoxigraph.NamedNode(EX + "s"), pyoxigraph.NamedNode(EX + "p"), pyoxigraph.Literal("o")))
assert (work / "oxilite.sqlite").exists()

# --- Step 5: Cypher over the same data --------------------------------------------------
cy = {"base": EX, "prefixes": {"foaf": "http://xmlns.com/foaf/0.1/"}}
result = store.cypher(
    "MATCH (a)-[:`foaf:knows`]->(b) RETURN a.`foaf:name` AS who, b.`foaf:name` AS knows ORDER BY who", **cy
)
print(result.records)  # [{'who': 'Ada', 'knows': 'Grace'}, {'who': 'Grace', 'knows': 'Alan'}]
assert result.records == [{"who": "Ada", "knows": "Grace"}, {"who": "Grace", "knows": "Alan"}]

created = store.cypher(
    "MATCH (a {`foaf:name`: 'Alan'}) CREATE (a)-[:MENTORS {since: 2024}]->(:Engineer {`foaf:name`: 'Linus'})", **cy
)
print(created.stats.nodes_created, created.stats.relationships_created)  # 1 1
assert (created.stats.nodes_created, created.stats.relationships_created) == (1, 1)
assert store.query('ASK { ?n foaf:name "Linus" ; a ex:Engineer }', prefixes=PREFIXES)

# --- Step 6: Datalog --------------------------------------------------------------------
program = """
@prefix ex:   <http://example.org/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
reach(?x, ?y) :- foaf:knows(?x, ?y).
reach(?x, ?y) :- ex:MENTORS(?x, ?y).
reach(?x, ?z) :- reach(?x, ?y), reach(?y, ?z).
?- reach(ex:ada, ?who).
"""
reached = sorted(r["who"].value for r in store.datalog(program).records)
print(reached)
assert len(reached) == 3  # grace, alan, and the node Cypher created for Linus

# --- Step 7: reasoning ------------------------------------------------------------------
store.update(
    """PREFIX ex: <http://example.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
    INSERT DATA { ex:Engineer rdfs:subClassOf ex:Person . ex:Researcher rdfs:subClassOf ex:Person . }"""
)
count = lambda **opts: len(list(store.query("SELECT ?p WHERE { ?p a ex:Person }", prefixes=PREFIXES, **opts)))
print(count(), count(reasoning="rdfs"))  # 0 4
assert (count(), count(reasoning="rdfs")) == (0, 4)
inferred = store.materialize(engine="reasonable")
assert inferred > 0 and count(include_inferred=True) == 4

# --- Step 8: JSON-LD documents and credentials ------------------------------------------
docs = store.jsonld()
key = docs.put(
    '{"@context": {"name": "http://xmlns.com/foaf/0.1/name"}, "@id": "http://example.org/doc/1", "name": "Ada"}'
)
print(key, docs.get(key).stored_at.tzinfo)  # http://example.org/doc/1 UTC
assert store.query('ASK { GRAPH <http://example.org/doc/1> { ?s foaf:name "Ada" } }', prefixes=PREFIXES)

credential = {
    "@context": ["https://www.w3.org/ns/credentials/v2"],
    "id": "urn:uuid:badge-1",
    "type": ["VerifiableCredential"],
    "issuer": "did:example:academy",
    "validFrom": "2024-01-01T00:00:00Z",
    "credentialSubject": {"id": "did:example:ada"},
}
vcs = store.credentials()
vcs.put(credential)
valid = vcs.find(issuer="did:example:academy", valid_at=datetime.now(timezone.utc))
print([c.key for c in valid])  # ['urn:uuid:badge-1']
assert [c.key for c in valid] == ["urn:uuid:badge-1"]

# --- Step 9: time travel ----------------------------------------------------------------
tasks = Store(work / "tasks.sqlite", versioning="log")
with tasks.commit(author="ada", message="open the ticket"):
    tasks.update('INSERT DATA { <http://example.org/t1> <http://example.org/status> "open" }')
with tasks.commit(author="grace", message="close it"):
    tasks.update(
        'DELETE DATA { <http://example.org/t1> <http://example.org/status> "open" } ;'
        ' INSERT DATA { <http://example.org/t1> <http://example.org/status> "done" }'
    )
status = "SELECT ?s WHERE { <http://example.org/t1> <http://example.org/status> ?s }"
now = [s["s"].value for s in tasks.query(status)]
before = [s["s"].value for s in tasks.query(status, as_of="HEAD~1")]
print(now, before)  # ['done'] ['open']
assert (now, before) == (["done"], ["open"])
print([(c.author, c.message) for c in tasks.history() if c.kind == "write"])
assert [(c.author, c.message) for c in tasks.history() if c.kind == "write"][:2] == [
    ("grace", "close it"),
    ("ada", "open the ticket"),
]
for change in tasks.diff("HEAD~1"):
    print("+" if change.added else "-", change.quad)
assert len(tasks.diff("HEAD~1")) == 2

# --- Step 10: threads and asyncio -------------------------------------------------------


async def main() -> int:
    # Store calls release the GIL, so a thread per call keeps the event loop free.
    results = await asyncio.gather(
        *(asyncio.to_thread(lambda: len(list(store.query("SELECT * WHERE { ?s ?p ?o }")))) for _ in range(4))
    )
    return results[0]


assert asyncio.run(main()) > 0
print("tour complete")

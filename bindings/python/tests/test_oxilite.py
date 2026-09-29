"""oxilite's extensions through the Python package: stores, reasoning, explain, Cypher, Datalog,
the schema registry, JSON-LD and credentials, versioning, text search, threads, and a SQLite
library loaded at runtime. The pyoxigraph API itself is covered by the `test_pyoxigraph_*` port.
"""

from __future__ import annotations

import os
import subprocess
import sys
import threading
from datetime import datetime, timezone
from io import BytesIO
from pathlib import Path

import pytest

from oxilite import (
    DEFAULT_GRAPH_IRI,
    BlankNode,
    CypherNode,
    CypherRelationship,
    DefaultGraph,
    JsonLdError,
    Literal,
    NamedNode,
    Quad,
    QuerySolutions,
    RdfFormat,
    Store,
    Variable,
)

REPO = Path(__file__).resolve().parents[3]
EX = "http://example.org/"
ex = lambda name: NamedNode(EX + name)  # noqa: E731
RDF_TYPE = NamedNode("http://www.w3.org/1999/02/22-rdf-syntax-ns#type")
SUBCLASS = NamedNode("http://www.w3.org/2000/01/rdf-schema#subClassOf")


def rows(result: object) -> list:
    assert isinstance(result, QuerySolutions)
    return list(result)


# @lat: [[tests#Python#File store persists across processes]]
def test_file_store_persists_across_processes(tmp_path: Path) -> None:
    db = tmp_path / "data.sqlite"
    code = (
        "from oxilite import Store, NamedNode, Quad\n"
        f"s = Store({str(db)!r})\n"
        "n = NamedNode('http://example.org/a')\n"
        "s.add(Quad(n, n, n))\n"
    )
    subprocess.run([sys.executable, "-c", code], check=True)
    n = ex("a")
    assert Quad(n, n, n) in Store(db)


# @lat: [[tests#Python#Directory and read-only stores]]
def test_directory_and_read_only_stores(tmp_path: Path) -> None:
    store = Store(tmp_path)
    store.add(Quad(ex("s"), ex("p"), Literal("o")))
    del store
    assert (tmp_path / "oxilite.sqlite").exists()
    ro = Store.read_only(tmp_path)
    assert list(ro) == [Quad(ex("s"), ex("p"), Literal("o"))]
    with pytest.raises(OSError):
        ro.add(Quad(ex("s"), ex("p"), Literal("other")))
    # backup writes a consistent copy that opens as a store.
    copy = tmp_path / "copy.sqlite"
    ro.backup(copy)
    assert len(Store(copy)) == 1


# @lat: [[tests#Python#Terms validate their input]]
def test_terms_validate_their_input() -> None:
    with pytest.raises(ValueError):
        NamedNode("not an iri")
    with pytest.raises(ValueError):
        BlankNode("not valid")
    with pytest.raises(ValueError):
        Literal("x", language="not a tag!")
    with pytest.raises(SyntaxError):
        Store().query("SELECT WHERE {")
    # Terms are values: hashable and usable as dict keys and in sets.
    assert {ex("a"), ex("a"), Literal(1)} == {ex("a"), Literal("1", datatype=NamedNode("http://www.w3.org/2001/XMLSchema#integer"))}


# @lat: [[tests#Python#Query-time reasoning and materialization]]
def test_reasoning() -> None:
    store = Store()
    store.extend([Quad(ex("rex"), RDF_TYPE, ex("Dog")), Quad(ex("Dog"), SUBCLASS, ex("Animal"))])
    ask = f"ASK {{ <{EX}rex> a <{EX}Animal> }}"
    assert not store.query(ask)
    assert store.query(ask, reasoning="rdfs")
    same_as = NamedNode("http://www.w3.org/2002/07/owl#sameAs")
    store.extend([Quad(ex("a"), same_as, ex("b")), Quad(ex("a"), ex("name"), Literal("A"))])
    q = f'ASK {{ <{EX}b> <{EX}name> "A" }}'
    counts = []
    for engine in ("sql", "reasonable"):
        counts.append(store.materialize(engine))
        assert not store.query(q)
        assert store.query(q, include_inferred=True)
    assert counts[0] == counts[1] > 0
    store.clear_inferences()
    assert not store.query(q, include_inferred=True)
    with pytest.raises(ValueError):
        store.materialize("magic")


# @lat: [[tests#Python#Explain returns SQL]]
def test_explain_returns_sql() -> None:
    store = Store()
    assert "SELECT" in store.explain("SELECT ?s WHERE { ?s ?p ?o }")
    assert store.explain_update("INSERT DATA { <http://a> <http://b> <http://c> }")


# @lat: [[tests#Python#Files round-trip through load and dump]]
def test_files_round_trip(tmp_path: Path) -> None:
    store = Store()
    store.load(
        "@prefix ex: <http://example.org/> . ex:a ex:p 1 . ex:b ex:p \"two\"@en .",
        RdfFormat.TURTLE,
        to_graph=ex("g"),
    )
    path = tmp_path / "data.nq"
    store.dump(path, RdfFormat.N_QUADS)
    copy = Store()
    copy.load(path=path)  # format from the extension
    assert set(copy) == set(store)
    out = BytesIO()
    copy.dump(out, RdfFormat.TURTLE, from_graph=ex("g"))
    assert b'"two"@en' in out.getvalue()
    bad = tmp_path / "bad.ttl"
    bad.write_text("@prefix ex: <http://example.org/> .\nex:a ex:p .\n")
    with pytest.raises(SyntaxError) as e:
        Store().load(path=bad)
    assert e.value.lineno == 2
    assert e.value.filename == str(bad)
    # Named-graph helpers.
    assert list(store.named_graphs()) == [ex("g")]
    assert store.contains_named_graph(ex("g"))
    store.clear_graph(ex("g"))
    assert len(store) == 0 and store.contains_named_graph(ex("g"))
    store.remove_graph(ex("g"))
    assert not store.contains_named_graph(ex("g"))


# @lat: [[tests#Python#Cypher reads and writes the same dataset]]
def test_cypher() -> None:
    store = Store()
    r = store.cypher(
        "CREATE (:Person {name: 'Grace'})-[:KNOWS {since: 1950}]->(:Person {name: 'Ada'})",
        base=EX,
    )
    assert r.stats.nodes_created == 2
    assert r.stats.relationships_created == 1
    assert store.query(f'ASK {{ ?p <{EX}name> "Ada" }}')
    found = store.cypher(
        "MATCH (a:Person {name: $name})-[k:KNOWS]->(b) RETURN b.name AS friend, a, k",
        {"name": "Grace"},
        base=EX,
    )
    assert found.columns == ["friend", "a", "k"]
    assert found.records[0]["friend"] == "Ada"
    a, k = found.records[0]["a"], found.records[0]["k"]
    assert isinstance(a, CypherNode) and "Person" in a.labels and a.properties["name"] == "Grace"
    assert isinstance(k, CypherRelationship) and k.rel_type == "KNOWS" and k.properties["since"] == 1950
    # Data inserted with SPARQL is visible to Cypher.
    store.update(f'INSERT DATA {{ <{EX}alan> a <{EX}Person> ; <{EX}name> "Alan" }}')
    names = store.cypher("MATCH (p:Person) RETURN p.name AS n ORDER BY n", base=EX)
    assert [row[0] for row in names.rows] == ["Ada", "Alan", "Grace"]
    assert "SELECT" in store.explain_cypher("MATCH (n:Person) RETURN n", base=EX)
    with pytest.raises(SyntaxError):
        store.cypher("MATCH (n RETURN n")


PREFIX = "@prefix ex: <http://example.org/> .\n"


def family() -> Store:
    s = Store()
    s.load(
        PREFIX + "ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy . ex:cy ex:parent ex:dee . "
        "ex:ada ex:age 42 . ex:bob ex:age 17 .",
        RdfFormat.TURTLE,
    )
    return s


# @lat: [[tests#Python#Synalog runs over the store]]
def test_synalog() -> None:
    store = family()
    program = (
        "# @table parent <http://example.org/parent>\n"
        "@Recursive(Ancestor, 10);\n"
        "Ancestor(x:, y:) distinct :- parent(subject: x, object: y);\n"
        "Ancestor(x:, y:) distinct :- Ancestor(x:, y: m), parent(subject: m, object: y);\n"
    )
    r = store.synalog(program, "Ancestor")
    assert r.columns == ["x", "y"]
    assert len(r.rows) == 6
    old = store.synalog(
        'Old(p:, age:) :- triples(subject: p, predicate: "http://example.org/age", object: age), age > 18;',
        "Old",
    )
    assert old.records == [{"p": EX + "ada", "age": 42}]
    page = store.synalog(
        '@OrderBy(P, "x");\nP(x:) :- par(subject: x);',
        "P",
        tables={"par": EX + "parent"},
        limit=1,
        offset=1,
    )
    assert page.rows == [[EX + "bob"]]
    assert "NOT MATERIALIZED" in store.synalog_sql(program, "Ancestor")
    with pytest.raises(NotImplementedError, match="ArgMax"):
        store.synalog("O(p? ArgMax= p -> a) distinct :- triples(subject: p, object: a);", "O")


# @lat: [[tests#Python#Datalog recursion and materialization]]
def test_datalog() -> None:
    store = family()
    r = store.datalog(
        PREFIX + "anc(?x, ?y) :- ex:parent(?x, ?y).\n"
        "anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).\n"
        "?- anc(ex:ada, ?who)."
    )
    assert r.columns == ["who"]
    assert sorted(row[0].value for row in r.rows) == [EX + "bob", EX + "cy", EX + "dee"]
    assert all(isinstance(rec["who"], NamedNode) for rec in r.records)
    assert r.rounds == []
    stats = store.datalog_materialize(
        PREFIX + "ex:ancestor(?x, ?y) :- ex:parent(?x, ?y).\n"
        "ex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z)."
    )
    assert (stats.relations, stats.inferred) == (1, 6)
    q = f"SELECT ?y WHERE {{ <{EX}ada> <{EX}ancestor> ?y }}"
    assert rows(store.query(q)) == []
    assert len(rows(store.query(q, include_inferred=True))) == 3
    assert "WITH RECURSIVE" in store.explain_datalog(
        PREFIX + "anc(?x, ?y) :- ex:parent(?x, ?y).\nanc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).\n?- anc(?x, ?y)."
    )
    with pytest.raises(ValueError, match="not stratified"):
        store.datalog(PREFIX + "p(?x) :- ex:parent(?x, ?y), not p(?y).\n?- p(?x).")


# @lat: [[tests#Python#Schema registry and system graphs]]
def test_schema_registry_and_system_graphs() -> None:
    store = Store()
    onto = ex("onto")
    store.add(Quad(ex("Dog"), SUBCLASS, ex("Animal"), onto))
    store.add(Quad(ex("rex"), RDF_TYPE, ex("Dog")))
    store.register_schema_graph(onto, "ontology", version="1.0", applies_to=[DefaultGraph()])
    [entry] = store.schema_graphs()
    assert (entry.graph, entry.role, entry.version) == (onto, "ontology", "1.0")
    assert entry.applies_to == [DEFAULT_GRAPH_IRI]
    assert store.query(f"ASK {{ <{EX}rex> a <{EX}Animal> }}", reasoning="rdfs")
    assert store.set_schema_graph_active(EX + "onto", False)
    assert not store.schema_graphs()[0].active
    assert store.unregister_schema_graph(onto)
    assert store.schema_graphs() == []
    with pytest.raises(ValueError):
        store.register_schema_graph(onto, "poetry")
    # Shapes feed the compiled shape index.
    shapes = ex("shapes")
    store.load(
        "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/> .\n"
        "ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;\n"
        "  sh:property [ sh:path ex:name ; sh:maxCount 1 ] .",
        RdfFormat.TURTLE,
        to_graph=shapes,
    )
    store.register_schema_graph(shapes, "shacl")
    [shape] = store.shape_index()
    assert (shape.target, shape.path, shape.max_count) == (EX + "Person", EX + "name", 1)
    assert store.drop_schema_graph(shapes) > 0
    # System graphs.
    vocabulary = "ASK { GRAPH <oxilite:vocabulary> { ?s ?p ?o } }"
    assert not Store().query(vocabulary)
    assert Store(system_graphs=True).query(vocabulary)
    plain = Store()
    assert plain.install_system_graphs()
    assert not plain.install_system_graphs()


PERSON = '{"@context": {"name": "http://schema.org/name"}, "@id": "urn:uuid:1234", "name": "Ada"}'


def fixture(name: str) -> str:
    return (REPO / "crates" / "oxilite-vc" / "tests" / "fixtures" / name).read_text(encoding="utf-8")


# @lat: [[tests#Python#JSON-LD documents round-trip and query]]
def test_jsonld_documents() -> None:
    store = Store()
    docs = store.jsonld()
    assert docs.put(PERSON) == "urn:uuid:1234"
    d = docs.get("urn:uuid:1234")
    assert d is not None and d.json == PERSON and d.document["name"] == "Ada"
    assert d.graph == NamedNode("urn:uuid:1234")
    assert d.stored_at is not None and d.stored_at.tzinfo is not None
    assert store.query('ASK { GRAPH <urn:uuid:1234> { ?s <http://schema.org/name> "Ada" } }')
    assert docs.document_for_graph(NamedNode("urn:uuid:1234")).key == "urn:uuid:1234"  # type: ignore[union-attr]
    assert docs.check() == []
    other = store.jsonld(key={"pointer": "/name"}, graph={"template": "https://example.org/g/{key}"})
    assert other.put({"@context": {"name": "http://schema.org/name"}, "name": "Alan"}) == "Alan"
    assert store.query("ASK { GRAPH <https://example.org/g/Alan> { ?s ?p ?o } }")
    assert [x.key for x in docs.list()] == ["Alan", "urn:uuid:1234"]
    assert docs.remove("urn:uuid:1234") and not docs.remove("urn:uuid:1234")
    with pytest.raises(JsonLdError) as e:
        docs.put({"name": "no id"})
    assert e.value.code == "missing-key"  # type: ignore[attr-defined]
    with pytest.raises(JsonLdError) as e:
        docs.put({"@context": "https://unknown.example/ctx", "@id": "urn:x"})
    assert e.value.code == "loading remote context failed"  # type: ignore[attr-defined]


# @lat: [[tests#Python#Credentials are stored and found]]
def test_credentials() -> None:
    store = Store()
    vcs = store.credentials()
    degree = fixture("degree-v2.json")
    assert vcs.put(degree) == "http://university.example/credentials/3732"
    stored = vcs.get("http://university.example/credentials/3732")
    assert stored is not None and stored.json == degree and stored.profile == "vc2"
    assert stored.valid_until == datetime(2040, 1, 1, tzinfo=timezone.utc)
    vp = vcs.put_presentation(fixture("presentation-v2.json"))
    assert vp.credentials == ["urn:uuid:vc-a", "urn:uuid:vc-b"]
    found = vcs.find(issuer="did:example:academy", valid_at=datetime(2024, 1, 1, tzinfo=timezone.utc))
    assert [d.key for d in found] == ["urn:uuid:vc-a", "urn:uuid:vc-b"]
    assert len(vcs.documents.graphs("http://university.example/credentials/3732")) == 2
    with pytest.raises(JsonLdError) as e:
        vcs.put({"@context": ["https://example.org"], "type": ["VerifiableCredential"]})
    assert e.value.code == "invalid"  # type: ignore[attr-defined]


STATUS = f"SELECT ?s WHERE {{ <{EX}t1> <{EX}status> ?s }}"


# @lat: [[tests#Python#Versioning and time travel]]
def test_versioning() -> None:
    store = Store(versioning="log")
    with store.commit(author="ada", message="seed"):
        store.update(f'INSERT DATA {{ <{EX}t1> <{EX}status> "open" }}')
    store.update(
        f'DELETE DATA {{ <{EX}t1> <{EX}status> "open" }} ; INSERT DATA {{ <{EX}t1> <{EX}status> "done" }}'
    )
    assert [r["s"].value for r in rows(store.query(STATUS))] == ["done"]
    assert [r["s"].value for r in rows(store.query(STATUS, as_of="HEAD~1"))] == ["open"]
    history = store.history()
    assert history[0].author is None  # the commit info ended with the `with` block
    seed = next(c for c in history if c.message == "seed")
    assert seed.author == "ada" and seed.time.tzinfo is not None
    diff = store.diff("HEAD~1")
    assert sorted((c.added, c.quad.object) for c in diff) == [(False, Literal("open")), (True, Literal("done"))]
    assert store.changes(0)
    assert store.datalog(f'?- <{EX}status>(?t, "open").', as_of="HEAD~1").rows
    assert store.cypher("MATCH (n) RETURN count(n) AS c", as_of="HEAD~1").rows
    store.purge(subject=ex("t1"), reason="erasure request")
    assert len(store) == 0
    assert [r["s"] for r in rows(store.query(STATUS, as_of="HEAD~1"))] == []
    status = store.set_versioning("stamped")
    assert (status.level, status.history) == ("stamped", "frozen")
    assert store.versioning().level == "stamped"
    with pytest.raises(Exception, match="history"):
        Store().query(STATUS, as_of="HEAD~1")


# @lat: [[tests#Python#Threads share a store]]
def test_threads_share_a_store() -> None:
    store = Store()

    def insert(worker: int) -> None:
        for i in range(100):
            store.add(Quad(ex(f"w{worker}"), ex("n"), Literal(i)))

    threads = [threading.Thread(target=insert, args=(w,)) for w in range(4)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert len(store) == 400


# @lat: [[tests#Python#Text search]]
def test_text_search() -> None:
    store = Store(text_index=True)
    store.load(
        PREFIX + 'ex:a ex:label "graph databases on SQLite" . ex:b ex:label "relational tables" .',
        RdfFormat.TURTLE,
    )
    q = (
        "PREFIX oxl: <https://oxilite.dev/ns#> SELECT ?x WHERE "
        '{ ?x <http://example.org/label> ?l FILTER(oxl:textMatch(?l, "graph data*")) }'
    )
    assert [r["x"] for r in rows(store.query(q))] == [ex("a")]
    assert "CREATE VIRTUAL TABLE" in Store.schema_sql(text_index=True)
    # Substitutions and prefixes on top of the compiled query.
    solutions = rows(store.query("SELECT ?x ?l WHERE { ?x ex:label ?l }", prefixes={"ex": EX}, substitutions={Variable("x"): ex("b")}))
    assert [s["l"] for s in solutions] == [Literal("relational tables")]


LIBRARY = os.environ.get("OXILITE_SQLITE_LIBRARY") or next(
    (p for p in ("/usr/lib/libsqlite3.dylib", "/usr/lib/x86_64-linux-gnu/libsqlite3.so.0") if os.path.exists(p)),
    None,
)


# @lat: [[tests#Python#A SQLite library loaded at runtime]]
@pytest.mark.skipif(LIBRARY is None, reason="no system libsqlite3 found; set OXILITE_SQLITE_LIBRARY")
def test_sqlite_library_loaded_at_runtime(tmp_path: Path) -> None:
    db = tmp_path / "lib.sqlite"
    store = Store(db, library=LIBRARY)
    store.update(f'INSERT DATA {{ <{EX}a> <{EX}p> "via the system SQLite" }}')
    assert len(rows(store.query("SELECT * WHERE { ?s ?p ?o }"))) == 1
    del store
    assert len(Store(db)) == 1  # the same file opens with the bundled SQLite

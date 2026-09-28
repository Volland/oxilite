"""Datalog over the store: recursive rules, compiled to recursive SQL, and materialized inferences.

Predicates with a prefix (ex:parent) read triples; other predicates (anc) are rules. `?-` is the goal.

    python examples/python/03_datalog.py
"""

from oxilite import NamedNode, RdfFormat, Store

store = Store()
store.load(
    """
    @prefix ex: <http://example.org/> .
    ex:ada   ex:parent ex:byron .
    ex:byron ex:parent ex:catherine .
    ex:catherine ex:parent ex:william .
    ex:ada   ex:name "Ada" .
    """,
    RdfFormat.TURTLE,
)

ANCESTORS = """
@prefix ex: <http://example.org/> .
anc(?x, ?y) :- ex:parent(?x, ?y).
anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).
?- anc(ex:ada, ?who).
"""

# A linear recursion runs as one recursive SQL statement.
result = store.datalog(ANCESTORS)
print(result.columns)  # ['who']
ancestors = sorted(r["who"].value.rsplit("/", 1)[1] for r in result.records)
print(ancestors)  # ['byron', 'catherine', 'william']
assert ancestors == ["byron", "catherine", "william"]

# Stratified negation and aggregation: who has no recorded parent, and how many ancestors each has.
roots = store.datalog(
    """
    @prefix ex: <http://example.org/> .
    person(?x) :- ex:parent(?x, _).
    person(?y) :- ex:parent(_, ?y).
    has_parent(?x) :- ex:parent(?x, _).
    root(?x) :- person(?x), not has_parent(?x).
    ?- root(?x).
    """
)
assert [r["x"].value for r in roots.records] == ["http://example.org/william"]

counts = store.datalog(
    """
    @prefix ex: <http://example.org/> .
    anc(?x, ?y) :- ex:parent(?x, ?y).
    anc(?x, ?z) :- ex:parent(?x, ?y), anc(?y, ?z).
    n(?x, COUNT(?y)) :- anc(?x, ?y).
    ?- n(?who, ?count).
    """
)
by_person = {r["who"].value.rsplit("/", 1)[1]: int(r["count"].value) for r in counts.records}
print(by_person)  # {'ada': 3, 'byron': 2, 'catherine': 1}
assert by_person == {"ada": 3, "byron": 2, "catherine": 1}

# See the strata, the strategy for each recursive component, and the SQL.
print(store.explain_datalog(ANCESTORS)[:300])

# Materialize rule conclusions as inferences: a rule head with a prefix writes triples.
stats = store.datalog_materialize(
    """
    @prefix ex: <http://example.org/> .
    ex:ancestor(?x, ?y) :- ex:parent(?x, ?y).
    ex:ancestor(?x, ?z) :- ex:parent(?x, ?y), ex:ancestor(?y, ?z).
    """
)
print(stats.inferred)  # 6
assert stats.inferred == 6

# Inferences are stored apart from the data: queries opt in with include_inferred=True.
count = "SELECT (COUNT(*) AS ?n) WHERE { ?x <http://example.org/ancestor> ?y }"
plain = int(next(iter(store.query(count)))["n"].value)
inferred = int(next(iter(store.query(count, include_inferred=True)))["n"].value)
print(plain, inferred)  # 0 6
assert (plain, inferred) == (0, 6)

# Cypher sees them too, and clear_inferences() removes them.
store.clear_inferences()
assert int(next(iter(store.query(count, include_inferred=True)))["n"].value) == 0
assert NamedNode("http://example.org/ada") in {q.subject for q in store}
print("datalog complete")

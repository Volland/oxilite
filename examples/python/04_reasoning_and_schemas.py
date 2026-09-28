"""Reasoning and the schema registry: RDFS at query time, OWL 2 RL materialization, and ontologies and
SHACL shapes registered as schema graphs.

    python examples/python/04_reasoning_and_schemas.py
"""

from oxilite import DefaultGraph, NamedNode, RdfFormat, Store

EX = "http://example.org/"
PREFIXES = {"ex": EX}

store = Store()
store.load(
    """
    @prefix ex: <http://example.org/> .
    ex:rex   a ex:Dog .
    ex:tom   a ex:Cat .
    ex:ada   ex:owns ex:rex .
    """,
    RdfFormat.TURTLE,
)
onto = NamedNode(EX + "onto")
store.load(
    """
    @prefix ex:   <http://example.org/> .
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    @prefix owl:  <http://www.w3.org/2002/07/owl#> .
    ex:Dog rdfs:subClassOf ex:Animal .
    ex:Cat rdfs:subClassOf ex:Animal .
    ex:owns rdfs:range ex:Pet ; rdfs:domain ex:Person .
    ex:ownedBy owl:inverseOf ex:owns .
    """,
    RdfFormat.TURTLE,
    to_graph=onto,
)

# Register the ontology: reasoning then uses it for the graphs it applies to.
store.register_schema_graph(onto, "ontology", version="1.0", applies_to=[DefaultGraph()])
print([(e.graph.value, e.role, e.version) for e in store.schema_graphs()])
# [('http://example.org/onto', 'ontology', '1.0')]

ANIMALS = "SELECT ?a WHERE { ?a a ex:Animal } ORDER BY ?a"
names = lambda results: [s["a"].value.rsplit("/", 1)[1] for s in results]  # noqa: E731

# Without reasoning, nothing is typed ex:Animal.
assert names(store.query(ANIMALS, prefixes=PREFIXES)) == []

# At query time: the query is rewritten over the class hierarchy, and nothing is stored.
print(names(store.query(ANIMALS, prefixes=PREFIXES, reasoning="rdfs")))  # ['rex', 'tom']
assert names(store.query(ANIMALS, prefixes=PREFIXES, reasoning="rdfs")) == ["rex", "tom"]
assert store.query("ASK { ex:ada a ex:Person }", prefixes=PREFIXES, reasoning="rdfs")  # from rdfs:domain

# By materialization: the OWL 2 RL closure, stored apart from the data.
inferred = store.materialize(engine="reasonable")  # or engine="sql"
print(inferred > 0)  # True
assert store.query("ASK { ex:rex ex:ownedBy ex:ada }", prefixes=PREFIXES, include_inferred=True)  # owl:inverseOf
assert not store.query("ASK { ex:rex ex:ownedBy ex:ada }", prefixes=PREFIXES)
store.clear_inferences()

# SHACL shapes compile to a property-shape index, and Cypher writes are checked against them.
shapes = NamedNode(EX + "shapes")
store.load(
    """
    @prefix sh: <http://www.w3.org/ns/shacl#> .
    @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
    @prefix ex: <http://example.org/> .
    ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
        sh:property [ sh:path ex:name ; sh:datatype xsd:string ; sh:maxCount 1 ] .
    """,
    RdfFormat.TURTLE,
    to_graph=shapes,
)
store.register_schema_graph(shapes, "shacl")
[shape] = store.shape_index()
print(shape.target, shape.path, shape.max_count)  # http://example.org/Person http://example.org/name 1
assert (shape.target, shape.path, shape.max_count) == (EX + "Person", EX + "name", 1)

store.cypher("CREATE (:Person {name: 'Grace'})", base=EX)
try:
    store.cypher("CREATE (:Person {name: ['Ada', 'Augusta']})", base=EX)  # two names: sh:maxCount 1
    raise AssertionError("the shape should have refused the write")
except ValueError as error:
    print("refused:", str(error)[:80])

# Queries can leave the registered graphs out, and answer over the data alone.
graphs = "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } } ORDER BY ?g"
print([s["g"].value for s in store.query(graphs)])
# ['http://example.org/onto', 'http://example.org/shapes', 'oxilite:schema']: the registry is RDF too
assert len(list(store.query(graphs))) == 3
assert list(store.query(graphs, include_schema_graphs=False)) == []
print("reasoning complete")

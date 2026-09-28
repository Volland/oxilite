"""openCypher over RDF: write a property graph with Cypher, read it back with Cypher and SPARQL.

Nodes are IRIs, labels are rdf:type, properties are literal triples, and relationships are triples
(with an RDF 1.2 reifier when they carry properties), so every language sees the same data.

    python examples/python/02_cypher_property_graph.py
"""

from oxilite import CypherNode, Store

store = Store()
opts = {"base": "http://example.org/"}  # the namespace of labels, types and keys without a prefix

# Writes are atomic and report what they changed.
created = store.cypher(
    """
    CREATE (ada:Person {name: 'Ada', born: 1815}),
           (grace:Person {name: 'Grace', born: 1906}),
           (alan:Person {name: 'Alan', born: 1912}),
           (ada)-[:INSPIRED {via: 'notes'}]->(grace),
           (grace)-[:KNOWS]->(alan)
    """,
    **opts,
)
print(created.stats.nodes_created, created.stats.relationships_created)  # 3 2
assert (created.stats.nodes_created, created.stats.relationships_created) == (3, 2)

# Parameters are JSON values; records are dicts keyed by column.
r = store.cypher(
    "MATCH (p:Person) WHERE p.born > $year RETURN p.name AS name ORDER BY name", {"year": 1900}, **opts
)
print(r.records)  # [{'name': 'Alan'}, {'name': 'Grace'}]
assert r.records == [{"name": "Alan"}, {"name": "Grace"}]

# Relationship properties, and variable-length paths.
r = store.cypher("MATCH (a)-[i:INSPIRED]->(b) RETURN a.name AS a, i.via AS via, b.name AS b", **opts)
assert r.records == [{"a": "Ada", "via": "notes", "b": "Grace"}]

r = store.cypher(
    "MATCH (a:Person {name: 'Ada'})-[*1..3]->(x) RETURN x.name AS reached ORDER BY reached", **opts
)
print([row["reached"] for row in r.records])  # ['Alan', 'Grace']
assert [row["reached"] for row in r.records] == ["Alan", "Grace"]

# Graph values come back as dataclasses; a node's id is its IRI.
node = store.cypher("MATCH (p:Person {name: 'Alan'}) RETURN p", **opts).rows[0][0]
assert isinstance(node, CypherNode)
print(node.labels, node.properties)  # ['Person'] {'born': 1912, 'name': 'Alan'}
assert node.properties == {"name": "Alan", "born": 1912}

# Aggregation.
r = store.cypher("MATCH (p:Person) RETURN count(p) AS people, min(p.born) AS earliest", **opts)
assert r.records == [{"people": 3, "earliest": 1815}]

# SPARQL sees what Cypher wrote.
solutions = store.query(
    "PREFIX ex: <http://example.org/> SELECT ?name WHERE { ?p a ex:Person ; ex:name ?name } ORDER BY ?name"
)
names = [s["name"].value for s in solutions]
print(names)  # ['Ada', 'Alan', 'Grace']
assert names == ["Ada", "Alan", "Grace"]

# Updates and deletes.
store.cypher("MATCH (p:Person {name: 'Alan'}) SET p.field = 'computing'", **opts)
deleted = store.cypher("MATCH (p:Person {name: 'Grace'}) DETACH DELETE p", **opts)
assert deleted.stats.nodes_deleted == 1 and deleted.stats.relationships_deleted == 2

# See the SPARQL and SQL a statement compiles to.
print(store.explain_cypher("MATCH (p:Person) RETURN p.name", **opts)[:200])
print("cypher complete")

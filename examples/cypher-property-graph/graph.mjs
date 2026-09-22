// A property graph on oxilite, written and queried with Cypher, read back with SPARQL.
// `demo` runs unchanged on @oxilite/node (sync) and @oxilite/d1 (async): every call is awaited.
import assert from "node:assert";

const base = "http://example.com/";

export async function demo(store) {
  const cypher = (q, params = {}, options = {}) => store.cypher(q, params, { base, ...options });

  // 1. Build the graph: one statement, one atomic request.
  let r = await cypher(`
    CREATE (ada:Person {name: 'Ada', born: 1815}),
           (grace:Person {name: 'Grace', born: 1906}),
           (alan:Person {name: 'Alan', born: 1912}),
           (acme:Company {name: 'Acme'}),
           (ada)-[:KNOWS {since: 2019}]->(grace),
           (grace)-[:KNOWS]->(alan),
           (ada)-[:WORKS_AT {role: 'engineer'}]->(acme),
           (grace)-[:WORKS_AT]->(acme)`);
  assert.deepStrictEqual(
    [r.stats.nodesCreated, r.stats.relationshipsCreated, r.stats.propertiesSet, r.stats.labelsAdded],
    [4, 4, 9, 4],
  );

  // 2. It is RDF: SPARQL sees labels, properties and relationship properties on reifiers.
  const names = await store.query(`PREFIX ex: <${base}> SELECT ?name WHERE { ?p a ex:Person ; ex:name ?name } ORDER BY ?name`);
  assert.deepStrictEqual(names.map((b) => b.get("name").value), ["Ada", "Alan", "Grace"]);
  const reifiers = await store.query(`PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> SELECT ?r WHERE { ?r rdf:reifies ?t }`);
  assert.strictEqual(reifiers.length, 2, "only the two relationships with properties have reifiers");

  // 3. Relationship properties, with null where a relationship has none.
  r = await cypher(`MATCH (p:Person)-[w:WORKS_AT]->(:Company {name: 'Acme'})
                    RETURN p.name AS name, w.role AS role ORDER BY name`);
  assert.deepStrictEqual(r.records, [{ name: "Ada", role: "engineer" }, { name: "Grace", role: null }]);

  // 4. Relationship uniqueness: colleagues, never paired with themselves.
  r = await cypher(`MATCH (a:Person)-[:WORKS_AT]->(c)<-[:WORKS_AT]-(b) RETURN a.name, b.name ORDER BY a.name`);
  assert.deepStrictEqual(r.rows, [["Ada", "Grace"], ["Grace", "Ada"]]);

  // 5. Variable-length paths and shortest paths.
  r = await cypher(`MATCH (:Person {name: 'Ada'})-[:KNOWS*1..3]->(f) RETURN f.name ORDER BY f.name`);
  assert.deepStrictEqual(r.rows, [["Alan"], ["Grace"]]);
  r = await cypher(`MATCH p = shortestPath((a:Person {name: 'Ada'})-[:KNOWS*]-(b:Person {name: 'Alan'}))
                    RETURN length(p) AS hops, [n IN nodes(p) | n.name] AS via`);
  assert.deepStrictEqual(r.records, [{ hops: 2, via: ["Ada", "Grace", "Alan"] }]);

  // 6. Aggregation through WITH.
  r = await cypher(`MATCH (c:Company)<-[:WORKS_AT]-(p) WITH c, p ORDER BY p.name
                    WITH c, count(p) AS staff, collect(p.name) AS people
                    RETURN c.name AS company, staff, people`);
  assert.deepStrictEqual(r.records, [{ company: "Acme", staff: 2, people: ["Ada", "Grace"] }]);

  // 7. Nodes and relationships come back as values.
  r = await cypher(`MATCH (n:Person {name: 'Ada'})-[k:KNOWS]->() RETURN n, k`);
  const [node, rel] = r.rows[0];
  assert.deepStrictEqual([node.type, node.labels, node.properties], ["node", ["Person"], { born: 1815, name: "Ada" }]);
  assert.deepStrictEqual([rel.type, rel.relType, rel.properties], ["relationship", "KNOWS", { since: 2019 }]);

  // 8. Writes: MERGE with parameters, a refused DELETE, DETACH DELETE.
  const hires = [{ name: "Alan", role: "researcher" }, { name: "Linus", role: "kernel" }];
  r = await cypher(
    `MATCH (c:Company {name: 'Acme'})
     UNWIND $hires AS h
     MERGE (p:Person {name: h.name})
     CREATE (p)-[:WORKS_AT {role: h.role}]->(c)`,
    { hires },
  );
  assert.deepStrictEqual([r.stats.nodesCreated, r.stats.relationshipsCreated], [1, 2]);
  await assert.rejects(async () => cypher(`MATCH (p:Person {name: 'Grace'}) DELETE p`), /still has relationships/);
  r = await cypher(`MATCH (p:Person {name: 'Grace'}) DETACH DELETE p`);
  assert.deepStrictEqual([r.stats.nodesDeleted, r.stats.relationshipsDeleted], [1, 3]);
  r = await cypher(`MATCH (p:Person) RETURN p.name ORDER BY p.name`);
  assert.deepStrictEqual(r.rows, [["Ada"], ["Alan"], ["Linus"]]);
}

// RDF written as Turtle, read with Cypher: the ontology and the shapes apply.
export async function fromRdf(store) {
  const cypher = (q, options = {}) => store.cypher(q, {}, { base, ...options });
  await store.load(
    `@prefix ex: <${base}> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
     @prefix sh: <http://www.w3.org/ns/shacl#> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
     ex:Engineer rdfs:subClassOf ex:Person .
     ex:ada a ex:Engineer ; ex:name "Ada" ; ex:tag "math", "poetry" .
     ex:grace a ex:Person ; ex:name "Grace" .
     ex:ada ex:KNOWS ex:grace .
     ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
       sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ;
       sh:property [ sh:path ex:born ; sh:datatype xsd:integer ] .`,
    { format: "text/turtle" },
  );
  let r = await cypher(`MATCH (p:Person) RETURN p.name ORDER BY p.name`);
  assert.deepStrictEqual(r.rows, [["Grace"]]);
  r = await cypher(`MATCH (p:Person) RETURN p.name ORDER BY p.name`, { reasoning: "rdfs" });
  assert.deepStrictEqual(r.rows, [["Ada"], ["Grace"]]);
  r = await cypher(`MATCH (p {name: 'Ada'})-[:KNOWS]->(f) RETURN p AS ada, p.tag AS tags, f.name AS friend`);
  assert.strictEqual(r.records[0].ada.id, `${base}ada`);
  assert.deepStrictEqual([r.records[0].tags, r.records[0].friend], [["math", "poetry"], "Grace"]);

  const size = await store.size;
  await assert.rejects(async () => cypher(`CREATE (:Person {name: 'Linus', born: 'ninety-nine'})`), /must have datatype xsd:integer/);
  await assert.rejects(async () => cypher(`CREATE (:Person {born: 1969})`), /needs at least 1 value/);
  assert.strictEqual(await store.size, size, "a rejected write changes nothing");
}

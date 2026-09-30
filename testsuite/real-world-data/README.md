# Real-world dataset snapshots

Fixed snapshots of publicly published RDF, vendored so the tests in
`testsuite/tests/real_world_datasets.rs` are hermetic and reproducible (no network access at
test time, no drift when the live document changes). See
`lat.md/test-plan.md#Oxigraph compatibility harness#Real-world dataset snapshots`.

Fetched 2026-09-30.

| File | Source | Format | License |
|---|---|---|---|
| `foaf.rdf` | <http://xmlns.com/foaf/spec/index.rdf> | RDF/XML | CC BY 1.0 (FOAF project) |
| `skos.rdf` | <https://www.w3.org/2004/02/skos/core.rdf> | RDF/XML | W3C document license |
| `doap.rdf` | <https://raw.githubusercontent.com/ewilderj/doap/master/schema/doap.rdf> | RDF/XML | Apache-2.0 (DOAP schema) |
| `dcat.ttl` | <https://www.w3.org/ns/dcat3.ttl> | Turtle | W3C document license |
| `prov-o.ttl` | <https://www.w3.org/ns/prov-o.ttl> | Turtle | W3C document license |
| `wikidata-q42.ttl` | <https://www.wikidata.org/wiki/Special:EntityData/Q42.ttl> | Turtle | CC0 (Wikidata) |
| `dbpedia-douglas-adams.ttl` | <https://dbpedia.org/data/Douglas_Adams.ttl> | Turtle | CC BY-SA 3.0 / GFDL (DBpedia, derived from Wikipedia) |
| `dbpedia-douglas-adams.jsonld` | <https://dbpedia.org/data/Douglas_Adams.jsonld> | JSON-LD | CC BY-SA 3.0 / GFDL (DBpedia, derived from Wikipedia) |

These are four independent publishers (the FOAF project, the W3C, the Wikidata community and
DBpedia) and three serializations, chosen to exercise real-world parser and store quirks that
synthetic fixtures do not: mixed-case and unusual IRIs, OWL/RDFS vocabulary axioms, deep
multilingual literal sets, Wikidata's statement/qualifier/reference reification pattern, and
DBpedia's `dbo:wikiPageWikiLink` link graph. The DBpedia entity is vendored in two formats
(Turtle and JSON-LD) so the same real data exercises both parsers.

To refresh a snapshot, re-run the `curl` command for that row and update the fetch date above.

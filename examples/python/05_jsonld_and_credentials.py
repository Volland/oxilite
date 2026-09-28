"""JSON-LD documents and Verifiable Credentials: the JSON is kept byte for byte, and its RDF goes into a
named graph that SPARQL queries like any other.

    python examples/python/05_jsonld_and_credentials.py
"""

from datetime import datetime, timezone

from oxilite import NamedNode, Store

store = Store()

# --- JSON-LD documents -------------------------------------------------------------------
CONTEXT = {"name": "http://schema.org/name", "worksFor": {"@id": "http://schema.org/worksFor", "@type": "@id"}}
docs = store.jsonld(contexts={"https://example.org/ctx": {"@context": CONTEXT}})  # contexts served from memory

key = docs.put({"@context": CONTEXT, "@id": "https://example.org/people/ada", "name": "Ada"})
print(key)  # https://example.org/people/ada: the key is the document's @id
docs.put_all(
    [
        {"@context": "https://example.org/ctx", "@id": "https://example.org/people/grace", "name": "Grace",
         "worksFor": "https://example.org/navy"},
        '{"@context": "https://example.org/ctx", "@id": "https://example.org/people/alan", "name": "Alan"}',
    ]
)

stored = docs.get("https://example.org/people/grace")
assert stored is not None
print(stored.json[:40], stored.stored_at.tzinfo)  # the exact text you stored, and a UTC timestamp
assert stored.document["worksFor"] == "https://example.org/navy"

# Each document's RDF is in its own graph, by default named by the key.
solutions = store.query(
    """SELECT ?g ?name WHERE { GRAPH ?g { ?p <http://schema.org/name> ?name } } ORDER BY ?name"""
)
rows = [(s["name"].value, s["g"].value.rsplit("/", 1)[1]) for s in solutions]
print(rows)  # [('Ada', 'ada'), ('Alan', 'alan'), ('Grace', 'grace')]
assert rows == [("Ada", "ada"), ("Alan", "alan"), ("Grace", "grace")]

# From a SPARQL graph back to its document.
hit = next(iter(store.query('SELECT ?g WHERE { GRAPH ?g { ?p <http://schema.org/worksFor> ?o } }')))
owner = docs.document_for_graph(hit["g"])
assert owner is not None and owner.key == "https://example.org/people/grace"

# Keyset paging, removal (which drops the document's graphs), and a drift check.
assert [d.key.rsplit("/", 1)[1] for d in docs.list(limit=2)] == ["ada", "alan"]
assert docs.remove("https://example.org/people/alan")
assert not store.query('ASK { ?p <http://schema.org/name> "Alan" }', use_default_graph_as_union=True)
assert docs.check() == []

# --- Verifiable Credentials --------------------------------------------------------------
vcs = store.credentials()  # the W3C contexts are bundled, so no network is needed
badge = {
    "@context": ["https://www.w3.org/ns/credentials/v2"],
    "id": "urn:uuid:badge-1",
    "type": ["VerifiableCredential"],
    "issuer": "did:example:academy",
    "validFrom": "2024-01-01T00:00:00Z",
    "validUntil": "2030-01-01T00:00:00Z",
    "credentialSubject": {"id": "did:example:ada"},
}
expired = dict(badge, id="urn:uuid:badge-0", validUntil="2024-06-01T00:00:00Z")
vcs.put(badge)
vcs.put(expired)

# Issuer, subject and validity are indexed.
now = datetime.now(timezone.utc)
valid = [c.key for c in vcs.find(issuer="did:example:academy", valid_at=now)]
print(valid)  # ['urn:uuid:badge-1']
assert valid == ["urn:uuid:badge-1"]
assert len(vcs.find(subject="did:example:ada")) == 2

stored = vcs.get("urn:uuid:badge-1")
assert stored is not None and stored.profile == "vc2" and stored.issuer == "did:example:academy"

# The credential is RDF too: query who holds a credential from which issuer.
q = """
PREFIX cred: <https://www.w3.org/2018/credentials#>
SELECT ?vc ?subject WHERE { GRAPH ?vc { ?vc cred:issuer <did:example:academy> ; cred:credentialSubject ?subject } }
ORDER BY ?vc
"""
holders = [(s["vc"].value, s["subject"].value) for s in store.query(q)]
print(holders)
assert ("urn:uuid:badge-1", "did:example:ada") in holders
assert NamedNode("urn:uuid:badge-1") in store.named_graphs()
print("documents complete")

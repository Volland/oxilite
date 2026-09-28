"""Full-text search: an SQLite FTS5 index over literals, used from SPARQL with oxl:textMatch.

    python examples/python/07_full_text_search.py
"""

from oxilite import RdfFormat, Store

store = Store(text_index=True)
store.load(
    """
    @prefix ex: <http://example.org/> .
    ex:a ex:text "Graph databases store nodes and edges" .
    ex:b ex:text "SQLite is an embedded relational database" .
    ex:c ex:text "Knowledge graphs link data with meaning" .
    """,
    RdfFormat.TURTLE,
)

SEARCH = """
PREFIX oxl: <https://oxilite.dev/ns#>
PREFIX ex:  <http://example.org/>
SELECT ?doc WHERE { ?doc ex:text ?t FILTER(oxl:textMatch(?t, ?terms)) } ORDER BY ?doc
"""


def search(terms: str) -> list:
    return [s["doc"].value.rsplit("/", 1)[1] for s in store.query(SEARCH.replace("?terms", f'"{terms}"'))]


print(search("graph*"))  # ['a', 'c']: a prefix query
assert search("graph*") == ["a", "c"]
print(search("embedded database"))  # ['b']: every term must match
assert search("embedded database") == ["b"]
assert search("graph* OR sqlite") == ["a", "b", "c"]

# The plan shows the FTS5 index in use.
print(store.explain(SEARCH.replace("?terms", '"graph*"'))[:300])

# Without the index the same query still answers, through the fallback evaluator.
plain = Store()
plain.load('<http://example.org/a> <http://example.org/text> "Graph databases" .', RdfFormat.N_TRIPLES)
assert len(list(plain.query(SEARCH.replace("?terms", '"graph*"')))) == 1
print("full-text search complete")

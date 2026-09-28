"""oxilite: an Oxigraph-compatible SPARQL 1.1 store on SQLite, with openCypher, Datalog,
RDFS / OWL reasoning, SHACL-aware schemas, JSON-LD documents, Verifiable Credentials and
time travel.

The API is pyoxigraph's, so ``import oxilite as pyoxigraph`` runs code written for Oxigraph's
Python package on a single SQLite file::

    from oxilite import Store, NamedNode, Quad

    store = Store("data.sqlite")
    store.add(Quad(NamedNode("http://ex/s"), NamedNode("http://ex/p"), NamedNode("http://ex/o")))
    for solution in store.query("SELECT ?s WHERE { ?s ?p ?o }"):
        print(solution["s"])
"""

from importlib.metadata import PackageNotFoundError, version

from ._formats import QueryResultsFormat, RdfFormat
from ._io import (
    QueryBoolean,
    QuerySolution,
    QuerySolutions,
    QueryTriples,
    parse,
    parse_query_results,
    serialize,
)
from ._model import (
    BaseDirection,
    BlankNode,
    DefaultGraph,
    Literal,
    NamedNode,
    Quad,
    Triple,
    Variable,
)
from ._native import JsonLdError
from ._store import Credentials, GraphArg, JsonLdDocuments, Store
from ._types import (
    DEFAULT_GRAPH_IRI,
    OXL,
    SCHEMA_GRAPH,
    VOCABULARY_GRAPH,
    Change,
    CommitRecord,
    CypherNode,
    CypherPath,
    CypherRelationship,
    CypherResult,
    CypherStats,
    CypherTemporal,
    DatalogMaterializeResult,
    DatalogResult,
    Drift,
    PresentationKeys,
    PropertyShapeEntry,
    SchemaGraphEntry,
    StoredDocument,
    VersionStatus,
)

try:
    __version__ = version("oxilite")
except PackageNotFoundError:  # pragma: no cover - running from a source tree
    __version__ = "0.0.0"

__all__ = [
    "DEFAULT_GRAPH_IRI",
    "OXL",
    "SCHEMA_GRAPH",
    "VOCABULARY_GRAPH",
    "BaseDirection",
    "BlankNode",
    "Change",
    "CommitRecord",
    "Credentials",
    "CypherNode",
    "CypherPath",
    "CypherRelationship",
    "CypherResult",
    "CypherStats",
    "CypherTemporal",
    "DatalogMaterializeResult",
    "DatalogResult",
    "DefaultGraph",
    "Drift",
    "GraphArg",
    "JsonLdDocuments",
    "JsonLdError",
    "Literal",
    "NamedNode",
    "PresentationKeys",
    "PropertyShapeEntry",
    "Quad",
    "QueryBoolean",
    "QueryResultsFormat",
    "QuerySolution",
    "QuerySolutions",
    "QueryTriples",
    "RdfFormat",
    "SchemaGraphEntry",
    "Store",
    "StoredDocument",
    "Triple",
    "Variable",
    "VersionStatus",
    "__version__",
    "parse",
    "parse_query_results",
    "serialize",
]

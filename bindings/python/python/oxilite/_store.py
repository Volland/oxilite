"""The oxilite `Store`: pyoxigraph's API on a SQLite file, plus every oxilite extension."""

from __future__ import annotations

import json
import os
from contextlib import contextmanager
from datetime import datetime
from typing import (
    Any,
    Callable,
    Dict,
    Iterable,
    Iterator,
    List,
    Mapping,
    Optional,
    Sequence,
    Union,
)

from . import _native
from ._formats import RdfFormat
from ._io import (
    Input,
    Output,
    PathArg,
    QueryResult,
    output_to_result,
    rdf_format,
    read_input,
    write_output,
)
from ._model import (
    BlankNode,
    DefaultGraph,
    GraphName,
    Literal,
    NamedNode,
    Quad,
    Subject,
    Term,
    Triple,
    Variable,
    as_quad,
    from_json,
    quad_from_json,
    to_json,
)
from ._types import (
    DEFAULT_GRAPH_IRI,
    Change,
    CommitRecord,
    CypherResult,
    DatalogMaterializeResult,
    DatalogResult,
    DocumentInput,
    Drift,
    PresentationKeys,
    PropertyShapeEntry,
    SchemaGraphEntry,
    StoredDocument,
    VersionStatus,
    changes,
    commit_record,
    cypher_param,
    cypher_result,
    datalog_result,
    property_shape_entry,
    schema_graph_entry,
    stored_document,
    version_status,
)

GraphArg = Union[NamedNode, BlankNode, DefaultGraph, str]
"""A graph argument: a term, or an IRI string."""


def _j(t: Any) -> Optional[str]:
    return None if t is None else json.dumps(to_json(t))


def _graph_json(g: GraphArg) -> str:
    return json.dumps({"termType": "NamedNode", "value": g} if isinstance(g, str) else to_json(g))


def _graph_iri(g: GraphArg) -> str:
    if isinstance(g, str):
        return g
    if isinstance(g, DefaultGraph):
        return DEFAULT_GRAPH_IRI
    return g.value


def _options(**kwargs: Any) -> str:
    """JSON options without the unset (None) ones."""
    return json.dumps({k: v for k, v in kwargs.items() if v is not None})


def _document_text(doc: DocumentInput) -> str:
    if isinstance(doc, str):
        return doc
    if isinstance(doc, bytes):
        return doc.decode()
    return json.dumps(doc)


def _timestamp(t: Union[datetime, float, int, None]) -> Optional[float]:
    if t is None:
        return None
    return t.timestamp() if isinstance(t, datetime) else float(t)


class Store:
    """An RDF dataset stored in SQLite, queryable with SPARQL, Cypher and Datalog.

    ``Store()`` is in memory; ``Store("data.sqlite")`` opens or creates a database file, and an
    existing directory holds its database in ``oxilite.sqlite`` (pyoxigraph's paths are
    directories). ``library`` loads a SQLite shared library instead of the bundled SQLite. The
    other options apply when the database is created.
    """

    def __init__(
        self,
        path: Optional[PathArg] = None,
        *,
        library: Optional[PathArg] = None,
        graph_index: bool = True,
        text_index: bool = False,
        versioning: str = "off",
        as_of_index: bool = False,
        stamp_index: bool = False,
        system_graphs: bool = False,
    ) -> None:
        options = json.dumps(
            {
                "graphIndex": graph_index,
                "textIndex": text_index,
                "versioning": versioning,
                "asOfIndex": as_of_index,
                "stampIndex": stamp_index,
                "systemGraphs": system_graphs,
            }
        )
        self._path = os.fspath(path) if path is not None else None
        self._native = _native.NativeStore(
            self._path, os.fspath(library) if library is not None else None, options, False
        )

    @classmethod
    def read_only(cls, path: PathArg) -> "Store":
        """Opens an existing store read-only: writes raise `OSError`."""
        store = cls.__new__(cls)
        store._path = os.fspath(path)
        store._native = _native.NativeStore(store._path, None, None, True)
        return store

    def __repr__(self) -> str:
        return f"<Store path={self._path or ':memory:'}>"

    # ------------------------------------------------------------------------ quad access

    def add(self, quad: Union[Quad, Triple]) -> None:
        """Adds a quad (a triple goes to the default graph)."""
        self._native.add(json.dumps([to_json(as_quad(quad))]), False)

    def extend(self, quads: Iterable[Union[Quad, Triple]]) -> None:
        """Adds quads atomically."""
        self._native.add(json.dumps([to_json(as_quad(q)) for q in quads]), False)

    def bulk_extend(self, quads: Iterable[Union[Quad, Triple]]) -> None:
        """Adds quads in chunks (not atomic) and refreshes planner statistics."""
        self._native.add(json.dumps([to_json(as_quad(q)) for q in quads]), True)

    def remove(self, quad: Union[Quad, Triple]) -> None:
        self._native.remove(json.dumps([to_json(as_quad(quad))]))

    def __contains__(self, quad: object) -> bool:
        if not isinstance(quad, (Quad, Triple)):
            return False
        return bool(self._native.contains(json.dumps(to_json(as_quad(quad)))))

    def __len__(self) -> int:
        return int(self._native.len())

    def __iter__(self) -> Iterator[Quad]:
        return self.quads_for_pattern(None, None, None, None)

    def quads_for_pattern(
        self,
        subject: Optional[Subject],
        predicate: Optional[NamedNode],
        object: Optional[Term],  # noqa: A002
        graph_name: Optional[GraphName] = None,
    ) -> Iterator[Quad]:
        """Quads matching a pattern; `None` matches anything (a `None` graph: every graph)."""
        out = self._native.quads_for_pattern(_j(subject), _j(predicate), _j(object), _j(graph_name))
        return iter([quad_from_json(q) for q in json.loads(out)])

    # ------------------------------------------------------------------------------ SPARQL

    def query(
        self,
        query: str,
        *,
        base_iri: Optional[str] = None,
        prefixes: Optional[Mapping[str, str]] = None,
        use_default_graph_as_union: bool = False,
        default_graph: Union[GraphName, Sequence[GraphName], None] = None,
        named_graphs: Optional[Sequence[Union[NamedNode, BlankNode]]] = None,
        substitutions: Optional[Mapping[Variable, Term]] = None,
        custom_functions: Optional[Mapping[NamedNode, Callable[..., Any]]] = None,
        custom_aggregate_functions: Optional[Mapping[NamedNode, Any]] = None,
        reasoning: Optional[str] = None,
        include_inferred: bool = False,
        include_schema_graphs: Optional[bool] = None,
        as_of: Optional[str] = None,
    ) -> QueryResult:
        """Runs a SPARQL query: `QuerySolutions` for SELECT, `QueryBoolean` for ASK,
        `QueryTriples` for CONSTRUCT and DESCRIBE.

        oxilite options: ``reasoning`` (``"rdfs"`` or ``"owl-ql"``) entails at query time,
        ``include_inferred`` also matches materialized inferences, ``include_schema_graphs=False``
        hides the registered ontology and shapes graphs, and ``as_of`` reads a past version
        (``"HEAD~1"``, ``"#42"``, ``"@2026-09-01T00:00:00Z"``) of a versioned store.
        """
        if custom_functions or custom_aggregate_functions:
            raise NotImplementedError(
                "oxilite compiles queries to SQL and cannot call Python functions during evaluation"
            )
        options: Dict[str, Any] = {
            "base_iri": base_iri,
            "prefixes": dict(prefixes) if prefixes else None,
            "use_default_graph_as_union": use_default_graph_as_union,
            "named_graphs": [to_json(g) for g in named_graphs] if named_graphs is not None else None,
            "reasoning": reasoning,
            "include_inferred": include_inferred,
            "include_schema_graphs": include_schema_graphs,
            "as_of": as_of,
        }
        if default_graph is not None:
            options["default_graph"] = (
                [to_json(g) for g in default_graph]
                if isinstance(default_graph, (list, tuple))
                else to_json(default_graph)  # type: ignore[arg-type]
            )
        if substitutions:
            options["substitutions"] = [[v.value, to_json(t)] for v, t in substitutions.items()]
        out = self._native.query(query, json.dumps({k: v for k, v in options.items() if v is not None}))
        return output_to_result(json.loads(out))

    def update(
        self,
        update: str,
        *,
        base_iri: Optional[str] = None,
        prefixes: Optional[Mapping[str, str]] = None,
    ) -> None:
        """Runs a SPARQL update, atomically."""
        self._native.update(update, base_iri, json.dumps(dict(prefixes)) if prefixes else None)

    def explain(self, query: str) -> str:
        """The SQL a query compiles to, with the planner's join order and notes."""
        return str(self._native.explain(query))

    def explain_update(self, update: str) -> str:
        """How an update runs: the SQL of each operation, or why it needs the fallback."""
        return str(self._native.explain_update(update))

    # -------------------------------------------------------------------------------- I/O

    def _load(
        self,
        input: Optional[Input],  # noqa: A002
        format: Union[RdfFormat, str, None],  # noqa: A002
        path: Optional[PathArg],
        base_iri: Optional[str],
        to_graph: Optional[GraphName],
        bulk: bool,
    ) -> None:
        f = rdf_format(format, path)
        self._native.load(
            read_input(input),
            os.fspath(path) if path is not None else None,
            f.media_type,
            base_iri,
            _j(to_graph),
            bulk,
        )

    def load(
        self,
        input: Optional[Input] = None,  # noqa: A002
        format: Union[RdfFormat, str, None] = None,  # noqa: A002
        *,
        path: Optional[PathArg] = None,
        base_iri: Optional[str] = None,
        to_graph: Optional[GraphName] = None,
    ) -> None:
        """Loads RDF atomically from a str, bytes, a file object or ``path=``; the format is
        guessed from the path's extension when not given."""
        self._load(input, format, path, base_iri, to_graph, False)

    def bulk_load(
        self,
        input: Optional[Input] = None,  # noqa: A002
        format: Union[RdfFormat, str, None] = None,  # noqa: A002
        *,
        path: Optional[PathArg] = None,
        base_iri: Optional[str] = None,
        to_graph: Optional[GraphName] = None,
    ) -> None:
        """Loads RDF in chunks (not atomic) and refreshes planner statistics."""
        self._load(input, format, path, base_iri, to_graph, True)

    def dump(
        self,
        output: Optional[Output] = None,
        format: Union[RdfFormat, str, None] = None,  # noqa: A002
        *,
        from_graph: Optional[GraphName] = None,
    ) -> Optional[bytes]:
        """Serializes the dataset (dataset formats), or one graph with ``from_graph``."""
        f = rdf_format(format, output)
        return write_output(output, self._native.dump(f.media_type, _j(from_graph)))

    # ------------------------------------------------------------------------------ graphs

    def named_graphs(self) -> Iterator[Union[NamedNode, BlankNode]]:
        return iter([from_json(g) for g in json.loads(self._native.named_graphs())])

    def contains_named_graph(self, graph_name: Union[NamedNode, BlankNode, DefaultGraph]) -> bool:
        if isinstance(graph_name, DefaultGraph):
            return True
        return bool(self._native.contains_named_graph(_graph_json(graph_name)))

    def add_graph(self, graph_name: Union[NamedNode, BlankNode, DefaultGraph]) -> None:
        if not isinstance(graph_name, DefaultGraph):
            self._native.add_graph(_graph_json(graph_name))

    def clear_graph(self, graph_name: GraphName) -> None:
        """Removes the quads of a graph, keeping the graph itself."""
        self._native.clear_graph(_graph_json(graph_name))

    def remove_graph(self, graph_name: GraphName) -> None:
        """Removes a named graph and its quads (the default graph is cleared)."""
        self._native.remove_graph(_graph_json(graph_name))

    def clear(self) -> None:
        self._native.clear()

    def flush(self) -> None:
        """A no-op kept for pyoxigraph compatibility: SQLite commits are durable."""
        self._native.flush()

    def optimize(self) -> None:
        """Refreshes planner statistics (run after large imports)."""
        self._native.optimize()

    def backup(self, target_directory: PathArg) -> None:
        """Writes a consistent copy of the database to a file (``VACUUM INTO``); a directory
        receives ``oxilite.sqlite``."""
        target = os.fspath(target_directory)
        if os.path.isdir(target):
            target = os.path.join(target, "oxilite.sqlite")
        self._native.backup(target)

    # ------------------------------------------------------------------------------ Cypher

    def cypher(
        self,
        query: str,
        params: Optional[Mapping[str, Any]] = None,
        *,
        base: Optional[str] = None,
        prefixes: Optional[Mapping[str, str]] = None,
        names: Optional[Mapping[str, str]] = None,
        multi_value: Optional[str] = None,
        var_length_cap: Optional[int] = None,
        shortest_path_cap: Optional[int] = None,
        shapes: Optional[bool] = None,
        as_of: Optional[str] = None,
        node_marker: Optional[bool] = None,
        reasoning: Optional[str] = None,
        use_default_graph_as_union: Optional[bool] = None,
    ) -> CypherResult:
        """Runs an openCypher statement over the property-graph view of the dataset: nodes are
        IRIs, labels ``rdf:type``, properties literal triples, relationships triples. A writing
        statement is applied atomically."""
        return cypher_result(
            json.loads(
                self._native.cypher(
                    query,
                    json.dumps(dict(params or {}), default=cypher_param),
                    self._cypher_options(
                        base, prefixes, names, multi_value, var_length_cap, shortest_path_cap,
                        shapes, as_of, node_marker, reasoning, use_default_graph_as_union,
                    ),
                )
            )
        )

    def explain_cypher(
        self,
        query: str,
        params: Optional[Mapping[str, Any]] = None,
        *,
        base: Optional[str] = None,
        prefixes: Optional[Mapping[str, str]] = None,
        names: Optional[Mapping[str, str]] = None,
        reasoning: Optional[str] = None,
        use_default_graph_as_union: Optional[bool] = None,
    ) -> str:
        """How a Cypher statement runs: its SPARQL, the SQL it compiles to, and what runs in
        Rust."""
        return str(
            self._native.explain_cypher(
                query,
                json.dumps(dict(params or {}), default=cypher_param),
                self._cypher_options(
                    base, prefixes, names, None, None, None, None, None, None, reasoning,
                    use_default_graph_as_union,
                ),
            )
        )

    @staticmethod
    def _cypher_options(
        base: Optional[str],
        prefixes: Optional[Mapping[str, str]],
        names: Optional[Mapping[str, str]],
        multi_value: Optional[str],
        var_length_cap: Optional[int],
        shortest_path_cap: Optional[int],
        shapes: Optional[bool],
        as_of: Optional[str],
        node_marker: Optional[bool],
        reasoning: Optional[str],
        use_default_graph_as_union: Optional[bool],
    ) -> str:
        return _options(
            base=base,
            prefixes=dict(prefixes) if prefixes else None,
            names=dict(names) if names else None,
            multiValue=multi_value,
            varLengthCap=var_length_cap,
            shortestPathCap=shortest_path_cap,
            shapes=shapes,
            asOf=as_of,
            nodeMarker=node_marker,
            reasoning=reasoning,
            useDefaultGraphAsUnion=use_default_graph_as_union,
        )

    # ----------------------------------------------------------------------------- Datalog

    def datalog(
        self,
        program: str,
        *,
        use_default_graph_as_union: Optional[bool] = None,
        include_inferred: Optional[bool] = None,
        max_iterations: Optional[int] = None,
        as_of: Optional[str] = None,
    ) -> DatalogResult:
        """Runs a Datalog program over the same quads: recursive rules with stratified negation,
        constraints and aggregation. A program whose recursion is linear is one SQL statement."""
        return datalog_result(
            json.loads(
                self._native.datalog(
                    program,
                    _options(
                        useDefaultGraphAsUnion=use_default_graph_as_union,
                        includeInferred=include_inferred,
                        maxIterations=max_iterations,
                        asOf=as_of,
                    ),
                )
            )
        )

    def datalog_materialize(
        self,
        program: str,
        *,
        use_default_graph_as_union: Optional[bool] = None,
        include_inferred: Optional[bool] = None,
        max_iterations: Optional[int] = None,
    ) -> DatalogMaterializeResult:
        """Stores what a program derives as inferences, where OWL 2 RL materialization writes
        them: SPARQL and Cypher see them with ``include_inferred``. Running either replaces the
        inference set."""
        out = json.loads(
            self._native.datalog_materialize(
                program,
                _options(
                    useDefaultGraphAsUnion=use_default_graph_as_union,
                    includeInferred=include_inferred,
                    maxIterations=max_iterations,
                ),
            )
        )
        return DatalogMaterializeResult(out["inferred"], out["relations"])

    def explain_datalog(self, program: str) -> str:
        """How a program runs: its strata, the strategy per recursive component, the SQL."""
        return str(self._native.explain_datalog(program))

    # --------------------------------------------------------------------------- reasoning

    def materialize(self, engine: str = "sql") -> int:
        """Computes the OWL 2 RL closure into the inference table (replacing earlier
        inferences); query it with ``include_inferred=True``. ``engine="reasonable"`` computes
        it in memory with the `reasonable` reasoner. Returns the number of inferred triples."""
        if engine not in ("sql", "reasonable"):
            raise ValueError(f"unknown engine {engine!r}: use 'sql' or 'reasonable'")
        return int(self._native.materialize(engine == "reasonable"))

    def clear_inferences(self) -> None:
        """Removes every materialized inference."""
        self._native.clear_inferences()

    # --------------------------------------------------------------------- schema registry

    def register_schema_graph(
        self,
        graph: GraphArg,
        role: str,
        *,
        iri: Optional[str] = None,
        version: Optional[str] = None,
        sha256: Optional[str] = None,
        imports: Optional[Sequence[str]] = None,
        applies_to: Optional[Sequence[GraphArg]] = None,
        active: bool = True,
    ) -> None:
        """Declares that a graph holds an ontology (``"ontology"``), SHACL shapes (``"shacl"``)
        or a ShEx schema (``"shex"``), and which graphs it applies to (every graph when
        ``applies_to`` is empty). The graph's triples stay where they are."""
        registration = {
            "iri": iri,
            "version": version,
            "sha256": sha256,
            "imports": list(imports) if imports is not None else None,
            "appliesTo": [_graph_iri(g) for g in applies_to] if applies_to is not None else None,
            "active": active,
        }
        self._native.register_schema_graph(
            _graph_json(graph), role, json.dumps({k: v for k, v in registration.items() if v is not None})
        )

    def schema_graphs(self) -> List[SchemaGraphEntry]:
        """The schema registry, ordered by role and graph."""
        return [schema_graph_entry(e) for e in json.loads(self._native.schema_graphs())]

    def set_schema_graph_active(self, graph: GraphArg, active: bool) -> bool:
        """Activates or deactivates a registration; returns whether one was found."""
        return bool(self._native.set_schema_graph_active(_graph_json(graph), active))

    def unregister_schema_graph(self, graph: GraphArg) -> bool:
        """Removes a registration, keeping the graph's triples; returns whether one was found."""
        return bool(self._native.unregister_schema_graph(_graph_json(graph)))

    def drop_schema_graph(self, graph: GraphArg) -> int:
        """Removes a registration and every quad of its graph; returns the number removed."""
        return int(self._native.drop_schema_graph(_graph_json(graph)))

    def shape_index(self) -> List[PropertyShapeEntry]:
        """The compiled SHACL property shapes of the registered shapes graphs."""
        return [property_shape_entry(e) for e in json.loads(self._native.shape_index())]

    def install_system_graphs(self) -> bool:
        """Installs or refreshes ``<oxilite:vocabulary>`` and the registry's description in
        ``<oxilite:schema>``; returns False when they were already current."""
        return bool(self._native.install_system_graphs())

    # ------------------------------------------------------------ JSON-LD and credentials

    def jsonld(
        self,
        *,
        key: Union[str, Mapping[str, str], None] = None,
        on_missing_key: Optional[str] = None,
        graph: Union[str, Mapping[str, str], None] = None,
        base_iri: Optional[str] = None,
        rdf_direction: Optional[str] = None,
        processing_mode: Optional[str] = None,
        contexts: Optional[Mapping[str, Any]] = None,
        network: bool = False,
        cache_fetched: bool = False,
        indexes: Optional[Mapping[str, bool]] = None,
    ) -> "JsonLdDocuments":
        """JSON-LD documents: each stored verbatim under a key (by default its ``@id``), its
        RDF in a named graph (by default the key) that SPARQL queries like any other graph.

        ``key``: ``"id"``, ``"contentHash"``, ``"explicit"`` or ``{"pointer": "/a/b"}``;
        ``graph``: ``"key"``, ``"default"``, ``{"template": "…{key}"}`` or ``{"fixed": iri}``;
        ``network=True`` downloads unknown contexts, which are otherwise an error.
        """
        options = self._jsonld_options(
            key, on_missing_key, graph, base_iri, rdf_direction, processing_mode, contexts,
            network, cache_fetched, indexes,
        )
        return JsonLdDocuments(lambda op, args: self._call(op, args, options))

    def credentials(
        self,
        *,
        key: Union[str, Mapping[str, str], None] = None,
        on_missing_key: Optional[str] = None,
        graph: Union[str, Mapping[str, str], None] = None,
        base_iri: Optional[str] = None,
        contexts: Optional[Mapping[str, Any]] = None,
        network: bool = False,
        cache_fetched: bool = False,
        indexes: Optional[Mapping[str, bool]] = None,
        embed_credentials: bool = True,
    ) -> "Credentials":
        """Verifiable Credentials (VCDM 1.1 and 2.0): stored verbatim under their ``id``, RDF in
        the graph of the same IRI, issuer, subject, type and validity indexed. The W3C contexts
        are bundled. Proofs are not verified."""
        options = self._jsonld_options(
            key, on_missing_key, graph, base_iri, None, None, contexts, network, cache_fetched, indexes
        )
        options.update(credentials=True, embedCredentials=embed_credentials)
        return Credentials(lambda op, args: self._call(op, args, options))

    @staticmethod
    def _jsonld_options(
        key: Union[str, Mapping[str, str], None],
        on_missing_key: Optional[str],
        graph: Union[str, Mapping[str, str], None],
        base_iri: Optional[str],
        rdf_direction: Optional[str],
        processing_mode: Optional[str],
        contexts: Optional[Mapping[str, Any]],
        network: bool,
        cache_fetched: bool,
        indexes: Optional[Mapping[str, bool]],
    ) -> Dict[str, Any]:
        idx = None
        if indexes is not None:
            idx = {("validUntil" if k == "valid_until" else k): v for k, v in indexes.items()}
        o = {
            "key": dict(key) if isinstance(key, Mapping) else key,
            "onMissingKey": on_missing_key,
            "graph": dict(graph) if isinstance(graph, Mapping) else graph,
            "baseIri": base_iri,
            "rdfDirection": rdf_direction,
            "processingMode": processing_mode,
            "contexts": dict(contexts) if contexts else None,
            "network": network or None,
            "cacheFetched": cache_fetched or None,
            "indexes": idx,
        }
        return {k: v for k, v in o.items() if v is not None}

    def _call(self, op: str, args: Dict[str, Any], options: Dict[str, Any]) -> Any:
        return json.loads(self._native.jsonld(op, json.dumps(args), json.dumps(options)))

    # ---------------------------------------------------------------------------- versioning

    def versioning(self) -> VersionStatus:
        """The versioning level and where the clock and history stand."""
        return version_status(json.loads(self._native.versioning()))

    def set_versioning(
        self,
        level: str,
        *,
        as_of_index: Optional[bool] = None,
        stamp_index: Optional[bool] = None,
        allow_loss: bool = False,
        author: Optional[str] = None,
        message: Optional[str] = None,
    ) -> VersionStatus:
        """Changes the level (``"off"``, ``"stamped"``, ``"log"``). Upgrades keep every quad (the
        upgrade to ``"log"`` records the whole store as its genesis); a downgrade freezes the
        history, and deletes it only with ``allow_loss=True``."""
        change = _options(
            asOfIndex=as_of_index,
            stampIndex=stamp_index,
            allowLoss=allow_loss,
            author=author,
            message=message,
        )
        return version_status(json.loads(self._native.set_versioning(level, change)))

    def set_commit_info(self, author: Optional[str] = None, message: Optional[str] = None) -> None:
        """Author and message recorded on the commits of the following writes (until changed)."""
        self._native.set_commit_info(_options(author=author, message=message))

    @contextmanager
    def commit(self, author: Optional[str] = None, message: Optional[str] = None) -> Iterator["Store"]:
        """Records ``author`` and ``message`` on the writes made inside the ``with`` block."""
        self.set_commit_info(author, message)
        try:
            yield self
        finally:
            self.set_commit_info()

    def history(self, limit: int = 20) -> List[CommitRecord]:
        """The latest commits and level changes, newest first."""
        return [commit_record(c) for c in json.loads(self._native.history(limit))]

    def changes(self, after: int = 0, until: Optional[int] = None) -> List[Change]:
        """The changes after tick ``after`` (up to ``until``), in order."""
        return changes(json.loads(self._native.changes(after, until)))

    def diff(self, from_: str, to: str = "HEAD") -> List[Change]:
        """The net difference between two versions (``"HEAD~1"``, ``"#42"``,
        ``"@2026-09-01T00:00:00Z"``)."""
        return changes(json.loads(self._native.diff(from_, to)))

    def purge(
        self,
        subject: Optional[Subject] = None,
        predicate: Optional[NamedNode] = None,
        object: Optional[Term] = None,  # noqa: A002
        graph: Optional[GraphName] = None,
        *,
        reason: Optional[str] = None,
    ) -> None:
        """Removes the matching quads from the store and from its whole history (audited)."""
        pattern = {
            k: to_json(v)
            for k, v in (("subject", subject), ("predicate", predicate), ("object", object), ("graph", graph))
            if v is not None
        }
        self._native.purge(json.dumps(pattern), reason)

    # -------------------------------------------------------------------------------- misc

    @staticmethod
    def schema_sql(*, graph_index: bool = True, text_index: bool = False, versioning: str = "off") -> str:
        """The database schema as a SQL script (for example a D1 migration)."""
        return str(
            _native.schema_sql(json.dumps({"graphIndex": graph_index, "textIndex": text_index, "versioning": versioning}))
        )


_Call = Callable[[str, Dict[str, Any]], Any]


class JsonLdDocuments:
    """JSON-LD documents of a store (see `Store.jsonld`). Every write is atomic."""

    def __init__(self, call: _Call) -> None:
        self._call = call

    def put(self, document: DocumentInput, key: Optional[str] = None) -> str:
        """Stores a document (replacing one with the same key) and returns its key. JSON text
        is stored byte for byte; dicts are serialized."""
        return self.put_all([(document, key)])[0]

    def put_all(self, documents: Iterable[Union[DocumentInput, "tuple[DocumentInput, Optional[str]]"]]) -> List[str]:
        """Stores several documents (each a document or a ``(document, key)`` pair) in one
        atomic write; returns their keys."""
        docs = []
        for d in documents:
            doc, key = d if isinstance(d, tuple) else (d, None)
            docs.append({"json": _document_text(doc), **({"key": key} if key is not None else {})})
        return list(self._call("put", {"documents": docs})["keys"])

    def get(self, key: str) -> Optional[StoredDocument]:
        """The stored document, or None."""
        return stored_document(self._call("get", {"key": key}))

    def remove(self, key: str) -> bool:
        """Removes a document and the graphs it owns; False when it was not stored."""
        return bool(self._call("remove", {"key": key}))

    def list(self, *, after: Optional[str] = None, limit: int = 100) -> List[StoredDocument]:
        """Documents ordered by key (keyset paging with ``after``)."""
        args: Dict[str, Any] = {"limit": limit}
        if after is not None:
            args["after"] = after
        return [d for d in map(stored_document, self._call("list", args)) if d is not None]

    def find(
        self,
        *,
        issuer: Optional[str] = None,
        subject: Optional[str] = None,
        type: Optional[str] = None,  # noqa: A002
        valid_at: Union[datetime, float, None] = None,
        profile: Optional[str] = None,
        after: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[StoredDocument]:
        """Documents matching metadata filters (indexed; no SPARQL)."""
        return [d for d in map(stored_document, self._call("find", _filter(issuer, subject, type, valid_at, profile, after, limit))) if d]

    def graphs(self, key: str) -> List[Union[NamedNode, BlankNode, DefaultGraph]]:
        """The graphs a document owns (its own graph and the graphs it defines, such as proofs)."""
        return [from_json(g) for g in self._call("graphs", {"key": key})]

    def document_for_graph(self, graph: GraphName) -> Optional[StoredDocument]:
        """The document behind a graph, such as a ``?g`` bound by SPARQL."""
        return stored_document(self._call("documentForGraph", {"graph": to_json(graph)}))

    def put_context(self, iri: str, context: Union[str, Mapping[str, Any]]) -> None:
        """Persists a context, so documents that reference ``iri`` convert offline."""
        self._call("putContext", {"iri": iri, "context": context if isinstance(context, str) else dict(context)})

    def remove_context(self, iri: str) -> None:
        self._call("removeContext", {"iri": iri})

    def contexts(self) -> List[str]:
        """IRIs of the persisted contexts."""
        return list(self._call("contexts", {}))

    def rebuild(self, key: str) -> bool:
        """Regenerates a document's graphs from its stored JSON; False when it is not stored."""
        return bool(self._call("rebuild", {"key": key}))

    def check(self) -> List[Drift]:
        """Documents whose graphs no longer match their JSON (after a SPARQL UPDATE, say)."""
        return [Drift(d["key"], d["missing"], d["extra"]) for d in self._call("check", {})]


def _filter(
    issuer: Optional[str],
    subject: Optional[str],
    type_: Optional[str],
    valid_at: Union[datetime, float, None],
    profile: Optional[str],
    after: Optional[str],
    limit: Optional[int],
) -> Dict[str, Any]:
    f = {
        "issuer": issuer,
        "subject": subject,
        "type": type_,
        "validAt": _timestamp(valid_at),
        "profile": profile,
        "after": after,
        "limit": limit,
    }
    return {k: v for k, v in f.items() if v is not None}


class Credentials:
    """Verifiable Credentials of a store (see `Store.credentials`)."""

    def __init__(self, call: _Call) -> None:
        self._call = call
        self.documents = JsonLdDocuments(call)
        """The document operations (graphs, contexts, check, rebuild…) with the bundled
        credential contexts."""

    def put(self, credential: DocumentInput, key: Optional[str] = None) -> str:
        """Checks and stores a credential; returns its key (its ``id``, or a content hash)."""
        args: Dict[str, Any] = {"json": _document_text(credential)}
        if key is not None:
            args["key"] = key
        return str(self._call("putCredential", args))

    def put_presentation(self, presentation: DocumentInput) -> PresentationKeys:
        """Stores a presentation and each credential it embeds, atomically."""
        out = self._call("putPresentation", {"json": _document_text(presentation)})
        return PresentationKeys(out["key"], list(out["credentials"]))

    def get(self, key: str) -> Optional[StoredDocument]:
        return self.documents.get(key)

    def remove(self, key: str) -> bool:
        return self.documents.remove(key)

    def find(
        self,
        *,
        issuer: Optional[str] = None,
        subject: Optional[str] = None,
        type: Optional[str] = None,  # noqa: A002
        valid_at: Union[datetime, float, None] = None,
        profile: Optional[str] = None,
        after: Optional[str] = None,
        limit: Optional[int] = None,
    ) -> List[StoredDocument]:
        """Credentials by issuer, subject, type, validity instant and profile (indexed)."""
        return [d for d in map(stored_document, self._call("find", _filter(issuer, subject, type, valid_at, profile, after, limit))) if d]


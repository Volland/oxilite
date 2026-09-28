"""Typed results of oxilite's extensions: Cypher, Datalog, JSON-LD documents and credentials,
versioning and the schema registry. They decode the JSON forms shared with `@oxilite/node`.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Union

from ._model import GraphName, Quad, from_json, quad_from_json


def _time(v: Optional[float]) -> Optional[datetime]:
    return None if v is None else datetime.fromtimestamp(v, tz=timezone.utc)


# ---------------------------------------------------------------------------------- Cypher


@dataclass(frozen=True)
class CypherNode:
    """A node of the property-graph view: an IRI (or ``_:label``), its labels and properties."""

    id: str
    labels: List[str]
    properties: Dict[str, Any]


@dataclass(frozen=True)
class CypherRelationship:
    """A relationship: an RDF triple, identified by its reifier when it has one."""

    id: str
    rel_type: str
    start: str
    end: str
    properties: Dict[str, Any]


@dataclass(frozen=True)
class CypherPath:
    nodes: List[CypherNode]
    relationships: List[CypherRelationship]


@dataclass(frozen=True)
class CypherTemporal:
    """A temporal value in its ISO 8601 form: ``type`` is ``date``, ``datetime``,
    ``localdatetime``, ``time``, ``localtime`` or ``duration``."""

    type: str
    value: str
    kind: str = ""

    def __str__(self) -> str:
        return self.value


@dataclass(frozen=True)
class CypherStats:
    """What a statement changed."""

    nodes_created: int = 0
    nodes_deleted: int = 0
    relationships_created: int = 0
    relationships_deleted: int = 0
    properties_set: int = 0
    labels_added: int = 0
    labels_removed: int = 0


@dataclass(frozen=True)
class CypherResult:
    """The result of a Cypher statement; `records` are the rows as dicts keyed by column."""

    columns: List[str]
    rows: List[List[Any]]
    records: List[Dict[str, Any]]
    stats: CypherStats


_TEMPORAL = {"date", "datetime", "localdatetime", "time", "localtime", "duration"}


def cypher_value(v: Any) -> Any:
    """A Cypher value from its JSON form: nodes, relationships, paths and temporal values are
    recognized by their exact keys, so a user map with a ``type`` key stays a dict."""
    if isinstance(v, list):
        return [cypher_value(x) for x in v]
    if not isinstance(v, dict):
        return v
    keys = set(v)
    t = v.get("type")
    if t == "node" and keys == {"type", "id", "labels", "properties"}:
        return CypherNode(v["id"], list(v["labels"]), cypher_value(v["properties"]))
    if t == "relationship" and keys == {"type", "id", "relType", "start", "end", "properties"}:
        return CypherRelationship(v["id"], v["relType"], v["start"], v["end"], cypher_value(v["properties"]))
    if t == "path" and keys == {"type", "nodes", "relationships"}:
        return CypherPath([cypher_value(n) for n in v["nodes"]], [cypher_value(r) for r in v["relationships"]])
    if t in _TEMPORAL and keys == {"type", "value", "kind"}:
        return CypherTemporal(t, v["value"], v["kind"])
    return {k: cypher_value(x) for k, x in v.items()}


def cypher_result(out: Dict[str, Any]) -> CypherResult:
    columns: List[str] = out["columns"]
    rows = [[cypher_value(c) for c in row] for row in out["rows"]]
    s = out["stats"]
    stats = CypherStats(
        nodes_created=s["nodesCreated"],
        nodes_deleted=s["nodesDeleted"],
        relationships_created=s["relationshipsCreated"],
        relationships_deleted=s["relationshipsDeleted"],
        properties_set=s["propertiesSet"],
        labels_added=s["labelsAdded"],
        labels_removed=s["labelsRemoved"],
    )
    return CypherResult(columns, rows, [dict(zip(columns, row)) for row in rows], stats)


def cypher_param(v: Any) -> Any:
    """JSON for values `json` cannot encode: temporal values as ISO strings, terms as their
    value."""
    if isinstance(v, datetime) or hasattr(v, "isoformat"):
        return v.isoformat()
    if isinstance(v, (CypherTemporal,)):
        return v.value
    value = getattr(v, "value", None)
    if isinstance(value, str):
        return value
    if isinstance(v, (set, frozenset, tuple)):
        return list(v)
    raise TypeError(f"cannot pass {type(v).__name__} as a Cypher parameter")


# --------------------------------------------------------------------------------- Datalog


@dataclass(frozen=True)
class DatalogResult:
    """The solutions of a Datalog program: terms, or None for an unbound column."""

    columns: List[str]
    rows: List[List[Any]]
    records: List[Dict[str, Any]]
    rounds: List[int]
    """Rounds each iterated component took; empty when nothing had to be iterated."""


@dataclass(frozen=True)
class DatalogMaterializeResult:
    inferred: int
    """Triples in the inference table afterwards."""
    relations: int
    """Relations whose conclusions were stored."""


def datalog_result(out: Dict[str, Any]) -> DatalogResult:
    columns: List[str] = out["columns"]
    rows = [[None if c is None else from_json(c) for c in row] for row in out["rows"]]
    return DatalogResult(columns, rows, [dict(zip(columns, row)) for row in rows], list(out["rounds"]))


# ------------------------------------------------------------------ JSON-LD and credentials


@dataclass(frozen=True)
class StoredDocument:
    """A stored JSON-LD document or credential."""

    key: str
    graph: GraphName
    """The graph its default-graph triples were written to."""
    json: str
    """The JSON exactly as it was stored."""
    sha256: str
    profile: str
    """``jsonld``, or ``vc1`` / ``vc2`` / ``vp1`` / ``vp2`` for credentials and presentations."""
    issuer: Optional[str]
    subject: Optional[str]
    types: List[str]
    valid_from: Optional[datetime]
    valid_until: Optional[datetime]
    refs: List[str]
    """Keys of the credentials a presentation embeds."""
    stored_at: Optional[datetime]

    @property
    def document(self) -> Any:
        """The stored JSON, parsed."""
        return json.loads(self.json)


def stored_document(j: Optional[Dict[str, Any]]) -> Optional[StoredDocument]:
    if j is None:
        return None
    return StoredDocument(
        key=j["key"],
        graph=from_json(j["graph"]),
        json=j["json"],
        sha256=j["sha256"],
        profile=j["profile"],
        issuer=j.get("issuer"),
        subject=j.get("subject"),
        types=list(j.get("types") or []),
        valid_from=_time(j.get("validFrom")),
        valid_until=_time(j.get("validUntil")),
        refs=list(j.get("refs") or []),
        stored_at=_time(j.get("storedAt")),
    )


@dataclass(frozen=True)
class Drift:
    """A document whose graphs differ from a fresh conversion of its JSON."""

    key: str
    missing: int
    extra: int


@dataclass(frozen=True)
class PresentationKeys:
    """Keys written by `Credentials.put_presentation`."""

    key: str
    credentials: List[str]


# ------------------------------------------------------------------------------ versioning


@dataclass(frozen=True)
class VersionStatus:
    """The versioning level of a store and where its clock and history stand."""

    level: str
    history: str
    stamp_column: bool
    stamp_index: bool
    as_of_index: bool
    head: Optional[int]
    head_time: Optional[datetime]
    genesis: Optional[int]
    frozen_at: Optional[int]
    commits: Optional[int]


def version_status(j: Dict[str, Any]) -> VersionStatus:
    return VersionStatus(
        level=j["level"],
        history=j.get("history", "none"),
        stamp_column=bool(j.get("stampColumn", False)),
        stamp_index=bool(j.get("stampIndex", False)),
        as_of_index=bool(j.get("asOfIndex", False)),
        head=j.get("head"),
        head_time=_time(j.get("headTime")),
        genesis=j.get("genesis"),
        frozen_at=j.get("frozenAt"),
        commits=j.get("commits"),
    )


@dataclass(frozen=True)
class CommitRecord:
    """One entry of the history: a commit (``write``) or a level change."""

    tick: int
    time: datetime
    kind: str
    author: Optional[str]
    message: Optional[str]
    added: Optional[int]
    removed: Optional[int]


def commit_record(j: Dict[str, Any]) -> CommitRecord:
    return CommitRecord(
        tick=j["tick"],
        time=_time(j["time"]) or datetime.fromtimestamp(0, tz=timezone.utc),
        kind=j["kind"],
        author=j.get("author"),
        message=j.get("message"),
        added=j.get("added"),
        removed=j.get("removed"),
    )


@dataclass(frozen=True)
class Change:
    """A quad added (``added=True``) or removed at a tick."""

    tick: int
    added: bool
    quad: Quad


def changes(v: List[Dict[str, Any]]) -> List[Change]:
    return [Change(c["tick"], c["added"], quad_from_json(c["quad"])) for c in v]


# ------------------------------------------------------------------------- schema registry

SCHEMA_GRAPH = "oxilite:schema"
"""The system graph holding the schema registry."""
VOCABULARY_GRAPH = "oxilite:vocabulary"
"""The system graph holding the oxilite vocabulary."""
OXL = "https://oxilite.dev/ns#"
"""The oxilite vocabulary namespace (``oxl:``)."""
DEFAULT_GRAPH_IRI = "https://oxilite.dev/ns#DefaultGraph"
"""Names the default graph in the registry (as a schema graph or an ``applies_to`` target)."""


@dataclass(frozen=True)
class SchemaGraphEntry:
    """One entry of the schema registry."""

    graph: GraphName
    role: str
    """``ontology``, ``shacl`` or ``shex``."""
    iri: Optional[str]
    version: Optional[str]
    sha256: Optional[str]
    imports: List[str]
    applies_to: List[str]
    """The graphs the schema applies to (IRIs, `DEFAULT_GRAPH_IRI`); empty: every graph."""
    active: bool
    loaded_at: Optional[str]


def schema_graph_entry(j: Dict[str, Any]) -> SchemaGraphEntry:
    return SchemaGraphEntry(
        graph=from_json(j["graph"]),
        role=j["role"],
        iri=j.get("iri"),
        version=j.get("version"),
        sha256=j.get("sha256"),
        imports=list(j.get("imports") or []),
        applies_to=list(j.get("appliesTo") or []),
        active=bool(j.get("active", True)),
        loaded_at=j.get("loadedAt"),
    )


@dataclass(frozen=True)
class PropertyShapeEntry:
    """One compiled SHACL property shape: the constraints on a target class and path."""

    target: str
    path: str
    datatype: Optional[str]
    min_count: Optional[int]
    max_count: Optional[int]
    pattern: Optional[str]
    values_in: List[Any]
    """The ``sh:in`` values."""
    relationship: bool
    """Relationship-valued (``sh:class`` / ``sh:node``)."""


def property_shape_entry(j: Dict[str, Any]) -> PropertyShapeEntry:
    return PropertyShapeEntry(
        target=j["target"],
        path=j["path"],
        datatype=j.get("datatype"),
        min_count=j.get("minCount"),
        max_count=j.get("maxCount"),
        pattern=j.get("pattern"),
        values_in=[from_json(t) for t in j.get("in") or []],
        relationship=bool(j.get("relationship", False)),
    )


DocumentInput = Union[str, bytes, Dict[str, Any], List[Any]]

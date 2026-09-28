"""RDF and SPARQL results formats, with pyoxigraph's API."""

from __future__ import annotations

from typing import Any, ClassVar, Dict, Optional, Tuple


class _Format:
    __slots__ = ("name", "iri", "media_type", "file_extension", "_aliases")
    _by_media_type: ClassVar[Dict[str, Any]]
    _by_extension: ClassVar[Dict[str, Any]]

    name: str
    iri: str
    _aliases: Tuple[str, ...]
    media_type: str
    file_extension: str

    def __init__(
        self, name: str, iri: str, media_type: str, file_extension: str, aliases: Tuple[str, ...] = ()
    ) -> None:
        object.__setattr__(self, "name", name)
        object.__setattr__(self, "iri", iri)
        object.__setattr__(self, "media_type", media_type)
        object.__setattr__(self, "file_extension", file_extension)
        object.__setattr__(self, "_aliases", aliases)

    def __setattr__(self, name: str, value: object) -> None:
        raise AttributeError(f"{type(self).__name__} is immutable")

    def __str__(self) -> str:
        return self.name

    def __repr__(self) -> str:
        return f"<{type(self).__name__} {self.name}>"

    def __eq__(self, other: object) -> bool:
        return (
            isinstance(other, _Format)
            and type(other) is type(self)
            and other.media_type == self.media_type
        )

    def __hash__(self) -> int:
        return hash((type(self), self.media_type))

    def __reduce__(self) -> Tuple[Any, ...]:
        return (type(self).from_media_type, (self.media_type,))

    @classmethod
    def _register(cls, *formats: "_Format") -> None:
        cls._by_media_type = {}
        cls._by_extension = {}
        for f in formats:
            cls._by_media_type[f.media_type] = f
            cls._by_extension[f.file_extension] = f
            for a in f._aliases:
                (cls._by_media_type if "/" in a else cls._by_extension)[a] = f

    @classmethod
    def from_media_type(cls, media_type: str) -> Optional[Any]:
        """The format of a media type (parameters such as ``; charset=utf-8`` are ignored)."""
        return cls._by_media_type.get(media_type.split(";")[0].strip().lower())

    @classmethod
    def from_extension(cls, extension: str) -> Optional[Any]:
        """The format of a file extension (``"ttl"``, ``"nq"``…)."""
        return cls._by_extension.get(extension.lower().lstrip("."))


class RdfFormat(_Format):
    """An RDF serialization format."""

    __slots__ = ("supports_datasets",)
    supports_datasets: bool

    N3: ClassVar["RdfFormat"]
    N_QUADS: ClassVar["RdfFormat"]
    N_TRIPLES: ClassVar["RdfFormat"]
    RDF_XML: ClassVar["RdfFormat"]
    TRIG: ClassVar["RdfFormat"]
    TURTLE: ClassVar["RdfFormat"]
    JSON_LD: ClassVar["RdfFormat"]

    def __init__(
        self,
        name: str,
        iri: str,
        media_type: str,
        file_extension: str,
        supports_datasets: bool,
        aliases: Tuple[str, ...] = (),
    ) -> None:
        super().__init__(name, iri, media_type, file_extension, aliases)
        object.__setattr__(self, "supports_datasets", supports_datasets)

    @property
    def supports_rdf_star(self) -> bool:
        """Whether the format can carry triple terms (RDF 1.2)."""
        return self.file_extension in ("nt", "nq", "ttl", "trig", "rdf")

    @classmethod
    def from_media_type(cls, media_type: str) -> Optional["RdfFormat"]:
        return super().from_media_type(media_type)

    @classmethod
    def from_extension(cls, extension: str) -> Optional["RdfFormat"]:
        return super().from_extension(extension)


RdfFormat.N3 = RdfFormat("N3", "http://www.w3.org/ns/formats/N3", "text/n3", "n3", False)
RdfFormat.N_QUADS = RdfFormat(
    "N-Quads", "http://www.w3.org/ns/formats/N-Quads", "application/n-quads", "nq", True, ("text/x-nquads",)
)
RdfFormat.N_TRIPLES = RdfFormat(
    "N-Triples", "http://www.w3.org/ns/formats/N-Triples", "application/n-triples", "nt", False, ("text/plain",)
)
RdfFormat.RDF_XML = RdfFormat(
    "RDF/XML", "http://www.w3.org/ns/formats/RDF_XML", "application/rdf+xml", "rdf", False, ("owl", "xml")
)
RdfFormat.TRIG = RdfFormat("TriG", "http://www.w3.org/ns/formats/TriG", "application/trig", "trig", True)
RdfFormat.TURTLE = RdfFormat(
    "Turtle", "http://www.w3.org/ns/formats/Turtle", "text/turtle", "ttl", False, ("application/x-turtle",)
)
RdfFormat.JSON_LD = RdfFormat(
    "JSON-LD", "http://www.w3.org/ns/formats/JSON-LD", "application/ld+json", "jsonld", True
)
RdfFormat._register(
    RdfFormat.N3,
    RdfFormat.N_QUADS,
    RdfFormat.N_TRIPLES,
    RdfFormat.RDF_XML,
    RdfFormat.TRIG,
    RdfFormat.TURTLE,
    RdfFormat.JSON_LD,
)


class QueryResultsFormat(_Format):
    """A SPARQL query results format."""

    __slots__ = ()

    CSV: ClassVar["QueryResultsFormat"]
    JSON: ClassVar["QueryResultsFormat"]
    TSV: ClassVar["QueryResultsFormat"]
    XML: ClassVar["QueryResultsFormat"]

    @classmethod
    def from_media_type(cls, media_type: str) -> Optional["QueryResultsFormat"]:
        return super().from_media_type(media_type)

    @classmethod
    def from_extension(cls, extension: str) -> Optional["QueryResultsFormat"]:
        return super().from_extension(extension)


QueryResultsFormat.CSV = QueryResultsFormat("SPARQL Results in CSV", "http://www.w3.org/ns/formats/SPARQL_Results_CSV", "text/csv", "csv")
QueryResultsFormat.JSON = QueryResultsFormat(
    "SPARQL Results in JSON",
    "http://www.w3.org/ns/formats/SPARQL_Results_JSON",
    "application/sparql-results+json",
    "srj",
    ("json", "application/json"),
)
QueryResultsFormat.TSV = QueryResultsFormat(
    "SPARQL Results in TSV",
    "http://www.w3.org/ns/formats/SPARQL_Results_TSV",
    "text/tab-separated-values",
    "tsv",
)
QueryResultsFormat.XML = QueryResultsFormat(
    "SPARQL Results in XML",
    "http://www.w3.org/ns/formats/SPARQL_Results_XML",
    "application/sparql-results+xml",
    "srx",
    ("xml", "application/xml"),
)
QueryResultsFormat._register(QueryResultsFormat.CSV, QueryResultsFormat.JSON, QueryResultsFormat.TSV, QueryResultsFormat.XML)

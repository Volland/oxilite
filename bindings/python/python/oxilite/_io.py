"""Query results, and the pyoxigraph I/O functions: ``parse``, ``serialize``,
``parse_query_results``.

Inputs are ``str``, ``bytes``, a file object or a ``path``; outputs are ``None`` (bytes are
returned), a binary file object or a path. Parsing and serializing run in Rust.
"""

from __future__ import annotations

import json
import os
from typing import IO, Any, Dict, Iterable, Iterator, List, Optional, Union

from . import _native
from ._formats import QueryResultsFormat, RdfFormat
from ._model import (
    BlankNode,
    Literal,
    NamedNode,
    Quad,
    Triple,
    Variable,
    as_quad,
    from_json,
    quad_from_json,
    to_json,
)

Input = Union[str, bytes, bytearray, memoryview, IO[bytes], IO[str]]
Output = Union[IO[bytes], str, "os.PathLike[str]"]
PathArg = Union[str, "os.PathLike[str]"]


def read_input(input: Optional[Input]) -> Optional[bytes]:  # noqa: A002
    """The bytes of an input (`None` when the data comes from a path)."""
    if input is None:
        return None
    if isinstance(input, str):
        return input.encode()
    if isinstance(input, (bytes, bytearray, memoryview)):
        return bytes(input)
    if hasattr(input, "read"):
        data = input.read()
        return data.encode() if isinstance(data, str) else bytes(data)
    raise TypeError(f"an input is a str, bytes or a file object, not {type(input).__name__}")


def write_output(output: Optional[Output], data: bytes) -> Optional[bytes]:
    """Writes to a file object or a path, or returns the bytes."""
    if output is None:
        return data
    if isinstance(output, (str, os.PathLike)):
        with open(output, "wb") as f:
            f.write(data)
        return None
    output.write(data)
    return None


def _extension(path: Optional[Union[PathArg, Output]]) -> Optional[str]:
    if isinstance(path, (str, os.PathLike)):
        ext = os.path.splitext(os.fspath(path))[1]
        return ext[1:] if ext else None
    return None


def rdf_format(format: Union[RdfFormat, str, None], path: Any = None) -> RdfFormat:  # noqa: A002
    """The RDF format: given, named by a media type or extension, or guessed from `path`."""
    if isinstance(format, RdfFormat):
        return format
    if isinstance(format, str):
        f = RdfFormat.from_media_type(format) or RdfFormat.from_extension(format)
        if f is None:
            raise ValueError(f"unknown RDF format {format!r}")
        return f
    ext = _extension(path)
    f = RdfFormat.from_extension(ext) if ext else None
    if f is None:
        raise ValueError("the RDF format is required: it cannot be guessed from the file extension")
    return f


def results_format(format: Union[QueryResultsFormat, str, None], path: Any = None) -> QueryResultsFormat:  # noqa: A002
    """The results format: given, named by a media type or extension, or guessed from `path`."""
    if isinstance(format, QueryResultsFormat):
        return format
    if isinstance(format, str):
        f = QueryResultsFormat.from_media_type(format) or QueryResultsFormat.from_extension(format)
        if f is None:
            raise ValueError(f"unknown query results format {format!r}")
        return f
    ext = _extension(path)
    f = QueryResultsFormat.from_extension(ext) if ext else None
    if f is None:
        raise ValueError("the query results format is required: it cannot be guessed from the file extension")
    return f


def _path(path: Optional[PathArg]) -> Optional[str]:
    return os.fspath(path) if path is not None else None


# ------------------------------------------------------------------------------- results


class QuerySolution:
    """One solution of a SELECT query: its values by index, name or `Variable`."""

    __slots__ = ("_variables", "_values")

    def __init__(self, variables: List[str], values: List[Any]) -> None:
        self._variables = variables
        self._values = values

    def __getitem__(self, key: Union[int, str, Variable]) -> Any:
        if isinstance(key, int):
            return self._values[key]
        name = key.value if isinstance(key, Variable) else key
        try:
            return self._values[self._variables.index(name)]
        except ValueError:
            raise KeyError(name) from None

    def get(self, key: Union[int, str, Variable], default: Any = None) -> Any:
        try:
            v = self[key]
        except (KeyError, IndexError):
            return default
        return default if v is None else v

    def __iter__(self) -> Iterator[Any]:
        return iter(self._values)

    def __len__(self) -> int:
        return len(self._values)

    def __eq__(self, other: object) -> bool:
        return (
            isinstance(other, QuerySolution)
            and other._variables == self._variables
            and other._values == self._values
        )

    def __hash__(self) -> int:
        return hash(tuple(self._values))

    def __repr__(self) -> str:
        pairs = " ".join(f"{v}={t}" for v, t in zip(self._variables, self._values) if t is not None)
        return f"<QuerySolution {pairs}>"


class QuerySolutions:
    """The solutions of a SELECT query: an iterator of `QuerySolution`s."""

    def __init__(self, output: Dict[str, Any]) -> None:
        self._output = output
        self._names: List[str] = output["variables"]
        self._rows = iter(output["rows"])

    @property
    def variables(self) -> List[Variable]:
        return [Variable._unchecked(n) for n in self._names]

    def __iter__(self) -> "QuerySolutions":
        return self

    def __next__(self) -> QuerySolution:
        row = next(self._rows)
        return QuerySolution(self._names, [None if t is None else from_json(t) for t in row])

    def serialize(
        self,
        output: Optional[Output] = None,
        format: Union[QueryResultsFormat, str, None] = None,  # noqa: A002
    ) -> Optional[bytes]:
        """Writes every solution (consumed or not) in a results format."""
        f = results_format(format, output)
        return write_output(output, _native.serialize_results(json.dumps(self._output), f.media_type))

    def __repr__(self) -> str:
        return f"<QuerySolutions variables={self._names}>"


class QueryBoolean:
    """The answer of an ASK query; usable as a `bool`."""

    __slots__ = ("_value",)

    def __init__(self, value: bool) -> None:
        self._value = value

    def __bool__(self) -> bool:
        return self._value

    def __eq__(self, other: object) -> bool:
        if isinstance(other, QueryBoolean):
            return other._value == self._value
        if isinstance(other, bool):
            return other == self._value
        return NotImplemented

    def __hash__(self) -> int:
        return hash(self._value)

    def __repr__(self) -> str:
        return f"<QueryBoolean {self._value}>"

    def serialize(
        self,
        output: Optional[Output] = None,
        format: Union[QueryResultsFormat, str, None] = None,  # noqa: A002
    ) -> Optional[bytes]:
        f = results_format(format, output)
        out = json.dumps({"kind": "boolean", "value": self._value})
        return write_output(output, _native.serialize_results(out, f.media_type))


class QueryTriples:
    """The triples of a CONSTRUCT or DESCRIBE query: an iterator of `Triple`s."""

    def __init__(self, quads: List[Dict[str, Any]]) -> None:
        self._quads = quads
        self._iter = iter(quads)

    def __iter__(self) -> "QueryTriples":
        return self

    def __next__(self) -> Triple:
        return quad_from_json(next(self._iter)).triple

    def serialize(
        self,
        output: Optional[Output] = None,
        format: Union[RdfFormat, str, None] = None,  # noqa: A002
    ) -> Optional[bytes]:
        """Writes every triple (consumed or not) in an RDF format."""
        f = rdf_format(format, output)
        return write_output(output, _native.serialize_rdf(json.dumps(self._quads), f.media_type))

    def __repr__(self) -> str:
        return f"<QueryTriples {len(self._quads)} triples>"


QueryResult = Union[QuerySolutions, QueryBoolean, QueryTriples]


def output_to_result(out: Dict[str, Any]) -> QueryResult:
    kind = out["kind"]
    if kind == "solutions":
        return QuerySolutions(out)
    if kind == "boolean":
        return QueryBoolean(out["value"])
    if kind == "quads":
        return QueryTriples(out["quads"])
    raise ValueError(f"unexpected output kind {kind}")


# ----------------------------------------------------------------------------- functions


def parse(
    input: Optional[Input] = None,  # noqa: A002
    format: Union[RdfFormat, str, None] = None,  # noqa: A002
    *,
    path: Optional[PathArg] = None,
    base_iri: Optional[str] = None,
    without_named_graphs: bool = False,
    rename_blank_nodes: bool = False,
    lenient: bool = False,
) -> Iterator[Quad]:
    """Parses RDF into quads (triples of graph formats are in the default graph).

    ``SyntaxError`` carries the file name and the position of the error.
    """
    f = rdf_format(format, path)
    out = _native.parse_rdf(
        read_input(input),
        _path(path),
        f.media_type,
        base_iri,
        without_named_graphs,
        rename_blank_nodes,
        lenient,
    )
    return iter([quad_from_json(q) for q in json.loads(out)])


def serialize(
    input: Iterable[Union[Triple, Quad]],  # noqa: A002
    output: Optional[Output] = None,
    format: Union[RdfFormat, str, None] = None,  # noqa: A002
) -> Optional[bytes]:
    """Serializes triples or quads; returns the bytes when `output` is None."""
    f = rdf_format(format, output)
    quads = json.dumps([to_json(as_quad(q)) for q in input])
    return write_output(output, _native.serialize_rdf(quads, f.media_type))


def parse_query_results(
    input: Optional[Input] = None,  # noqa: A002
    format: Union[QueryResultsFormat, str, None] = None,  # noqa: A002
    *,
    path: Optional[PathArg] = None,
) -> Union[QuerySolutions, QueryBoolean]:
    """Parses SPARQL query results (JSON, XML, CSV or TSV)."""
    f = results_format(format, path)
    out = json.loads(_native.parse_query_results(read_input(input), _path(path), f.media_type))
    result = output_to_result(out)
    assert isinstance(result, (QuerySolutions, QueryBoolean))
    return result


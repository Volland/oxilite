"""RDF terms with pyoxigraph's API, and their JSON form shared with the native module.

Terms are immutable values: hashable, comparable, picklable and usable in ``match``
statements. Constructors validate their input through the native module (``oxrdf``), so a bad
IRI fails where it is written; terms decoded from store results skip that check.
"""

from __future__ import annotations

import builtins
import enum
import math
from typing import Any, Dict, Iterator, Optional, Tuple, Union

from . import _native

XSD = "http://www.w3.org/2001/XMLSchema#"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
_XSD_STRING = XSD + "string"
_XSD_BOOLEAN = XSD + "boolean"
_XSD_INTEGER = XSD + "integer"
_XSD_DOUBLE = XSD + "double"
_RDF_LANG_STRING = RDF + "langString"
_RDF_DIR_LANG_STRING = RDF + "dirLangString"


def _escape(value: str) -> str:
    out = []
    for c in value:
        if c == '"':
            out.append('\\"')
        elif c == "\\":
            out.append("\\\\")
        elif c == "\n":
            out.append("\\n")
        elif c == "\r":
            out.append("\\r")
        elif c == "\t":
            out.append("\\t")
        elif c == "\b":
            out.append("\\b")
        elif c == "\f":
            out.append("\\f")
        elif ord(c) < 0x20 or ord(c) == 0x7F:
            out.append(f"\\u{ord(c):04X}")
        else:
            out.append(c)
    return "".join(out)


class _Term:
    __slots__ = ()

    def __setattr__(self, name: str, value: object) -> None:
        raise AttributeError(f"{type(self).__name__} is immutable")

    def __delattr__(self, name: str) -> None:
        raise AttributeError(f"{type(self).__name__} is immutable")

    def __copy__(self) -> "_Term":
        return self

    def __deepcopy__(self, memo: Dict[int, Any]) -> "_Term":
        return self


def _set(obj: object, **fields: object) -> None:
    for k, v in fields.items():
        object.__setattr__(obj, k, v)


class NamedNode(_Term):
    """An IRI, like ``<http://example.com/>``."""

    __slots__ = ("value",)
    __match_args__ = ("value",)
    value: str

    def __init__(self, value: str) -> None:
        _native.check_iri(value)
        _set(self, value=value)

    @classmethod
    def _unchecked(cls, value: str) -> "NamedNode":
        n = object.__new__(cls)
        _set(n, value=value)
        return n

    def __str__(self) -> str:
        return f"<{self.value}>"

    def __repr__(self) -> str:
        return f"<NamedNode value={self.value}>"

    def __eq__(self, other: object) -> bool:
        return type(other) is NamedNode and other.value == self.value

    def __hash__(self) -> int:
        return hash((NamedNode, self.value))

    def __reduce__(self) -> Tuple[Any, ...]:
        return (NamedNode._unchecked, (self.value,))


class BlankNode(_Term):
    """A blank node; without a value, a fresh random identifier is chosen."""

    __slots__ = ("value",)
    __match_args__ = ("value",)
    value: str

    def __init__(self, value: Optional[str] = None) -> None:
        if value is None:
            value = _native.new_blank_node_id()
        else:
            _native.check_blank_node(value)
        _set(self, value=value)

    @classmethod
    def _unchecked(cls, value: str) -> "BlankNode":
        n = object.__new__(cls)
        _set(n, value=value)
        return n

    def __str__(self) -> str:
        return f"_:{self.value}"

    def __repr__(self) -> str:
        return f"<BlankNode value={self.value}>"

    def __eq__(self, other: object) -> bool:
        return type(other) is BlankNode and other.value == self.value

    def __hash__(self) -> int:
        return hash((BlankNode, self.value))

    def __reduce__(self) -> Tuple[Any, ...]:
        return (BlankNode._unchecked, (self.value,))


class BaseDirection(enum.Enum):
    """The base direction of a directional language-tagged string (RDF 1.2)."""

    __match_args__ = ("value",)

    LTR = "ltr"
    RTL = "rtl"

    def __str__(self) -> str:
        return str(self.value)


class Literal(_Term):
    """A literal: a string, a language-tagged string or a typed value.

    ``Literal(True)``, ``Literal(1)`` and ``Literal(0.1)`` build ``xsd:boolean``,
    ``xsd:integer`` and ``xsd:double`` literals with Oxigraph's canonical lexical forms.
    """

    __slots__ = ("value", "datatype", "language", "direction")
    __match_args__ = ("value",)
    value: str
    datatype: NamedNode
    language: Optional[str]
    direction: Optional[BaseDirection]

    def __init__(
        self,
        value: Union[str, bool, int, float],
        *,
        datatype: Optional[NamedNode] = None,
        language: Optional[str] = None,
        direction: Optional[BaseDirection] = None,
    ) -> None:
        if isinstance(value, bool):
            lexical, dt = ("true" if value else "false"), _XSD_BOOLEAN
        elif isinstance(value, int):
            lexical, dt = str(value), _XSD_INTEGER
        elif isinstance(value, float):
            lexical, dt = _native.double_lexical(value if not math.isnan(value) else math.nan), _XSD_DOUBLE
        elif isinstance(value, str):
            lexical, dt = value, _XSD_STRING
        else:
            raise TypeError(f"a literal value is a str, bool, int or float, not {type(value).__name__}")
        if direction is not None and not isinstance(direction, BaseDirection):
            direction = BaseDirection(direction)
        if language is not None:
            _native.check_language(language)
            language = language.lower()
            expected = _RDF_DIR_LANG_STRING if direction is not None else _RDF_LANG_STRING
            if datatype is not None and datatype.value != expected:
                raise ValueError(f"a language-tagged literal has the datatype <{expected}>")
            dt = expected
        elif direction is not None:
            raise ValueError("a direction requires a language tag")
        elif datatype is not None:
            if not isinstance(value, str):
                raise ValueError("a datatype can only be given with a string value")
            if datatype.value in (_RDF_LANG_STRING, _RDF_DIR_LANG_STRING):
                raise ValueError(f"the datatype {datatype} requires a language tag")
            dt = datatype.value
        _set(
            self,
            value=lexical,
            datatype=datatype if datatype is not None and datatype.value == dt else NamedNode._unchecked(dt),
            language=language,
            direction=direction,
        )

    @classmethod
    def _unchecked(
        cls, value: str, datatype: str, language: Optional[str], direction: Optional[str]
    ) -> "Literal":
        n = object.__new__(cls)
        _set(
            n,
            value=value,
            datatype=NamedNode._unchecked(datatype),
            language=language or None,
            direction=BaseDirection(direction) if direction else None,
        )
        return n

    def __str__(self) -> str:
        v = f'"{_escape(self.value)}"'
        if self.language is not None:
            return f"{v}@{self.language}--{self.direction.value}" if self.direction else f"{v}@{self.language}"
        if self.datatype.value == _XSD_STRING:
            return v
        return f"{v}^^{self.datatype}"

    def __repr__(self) -> str:
        extra = (
            f" language={self.language}" + (f" direction={self.direction.value}" if self.direction else "")
            if self.language is not None
            else f" datatype={self.datatype.value}"
        )
        return f"<Literal value={self.value}{extra}>"

    def __eq__(self, other: object) -> bool:
        return (
            type(other) is Literal
            and other.value == self.value
            and other.datatype == self.datatype
            and other.language == self.language
            and other.direction == self.direction
        )

    def __hash__(self) -> int:
        return hash((Literal, self.value, self.datatype.value, self.language, self.direction))

    def __reduce__(self) -> Tuple[Any, ...]:
        return (
            Literal._unchecked,
            (self.value, self.datatype.value, self.language, self.direction.value if self.direction else None),
        )


class DefaultGraph(_Term):
    """The default graph of a dataset."""

    __slots__ = ()
    __match_args__ = ()

    @property
    def value(self) -> str:
        return ""

    def __str__(self) -> str:
        return "DEFAULT"

    def __repr__(self) -> str:
        return "<DefaultGraph>"

    def __eq__(self, other: object) -> bool:
        return type(other) is DefaultGraph

    def __hash__(self) -> int:
        return hash(DefaultGraph)

    def __reduce__(self) -> Tuple[Any, ...]:
        return (DefaultGraph, ())


class Variable(_Term):
    """A SPARQL variable, like ``?s``."""

    __slots__ = ("value",)
    __match_args__ = ("value",)
    value: str

    def __init__(self, value: str) -> None:
        _native.check_variable(value)
        _set(self, value=value)

    @classmethod
    def _unchecked(cls, value: str) -> "Variable":
        n = object.__new__(cls)
        _set(n, value=value)
        return n

    def __str__(self) -> str:
        return f"?{self.value}"

    def __repr__(self) -> str:
        return f"<Variable value={self.value}>"

    def __eq__(self, other: object) -> bool:
        return type(other) is Variable and other.value == self.value

    def __hash__(self) -> int:
        return hash((Variable, self.value))

    def __reduce__(self) -> Tuple[Any, ...]:
        return (Variable._unchecked, (self.value,))


Subject = Union[NamedNode, BlankNode, "Triple"]
Term = Union[NamedNode, BlankNode, Literal, "Triple"]
GraphName = Union[NamedNode, BlankNode, DefaultGraph]


def _inner(t: object) -> str:
    return f"<<( {t} )>>" if isinstance(t, Triple) else str(t)


class Triple(_Term):
    """An RDF triple; a triple can itself be the object of a triple (RDF 1.2)."""

    __slots__ = ("subject", "predicate", "object")
    __match_args__ = ("subject", "predicate", "object")
    subject: Subject
    predicate: NamedNode
    object: Term

    def __init__(self, subject: Subject, predicate: NamedNode, object: Term) -> None:  # noqa: A002
        if not isinstance(subject, (NamedNode, BlankNode, Triple)):
            raise TypeError(f"a triple subject is a NamedNode, BlankNode or Triple, not {type(subject).__name__}")
        if not isinstance(predicate, NamedNode):
            raise TypeError(f"a triple predicate is a NamedNode, not {type(predicate).__name__}")
        if not isinstance(object, (NamedNode, BlankNode, Literal, Triple)):
            raise TypeError(f"a triple object is a NamedNode, BlankNode, Literal or Triple, not {type(object).__name__}")
        _set(self, subject=subject, predicate=predicate, object=object)

    def __str__(self) -> str:
        return f"{_inner(self.subject)} {self.predicate} {_inner(self.object)}"

    def __repr__(self) -> str:
        return f"<Triple subject={self.subject!r} predicate={self.predicate!r} object={self.object!r}>"

    def __eq__(self, other: builtins.object) -> bool:
        return (
            type(other) is Triple
            and other.subject == self.subject
            and other.predicate == self.predicate
            and other.object == self.object
        )

    def __hash__(self) -> int:
        return hash((Triple, self.subject, self.predicate, self.object))

    def __getitem__(self, index: int) -> Any:
        return (self.subject, self.predicate, self.object)[index]

    def __len__(self) -> int:
        return 3

    def __iter__(self) -> Iterator[Any]:
        return iter((self.subject, self.predicate, self.object))

    def __reduce__(self) -> Tuple[Any, ...]:
        return (Triple, (self.subject, self.predicate, self.object))


class Quad(_Term):
    """A triple in a graph (the default graph when ``graph_name`` is omitted)."""

    __slots__ = ("subject", "predicate", "object", "graph_name")
    __match_args__ = ("subject", "predicate", "object", "graph_name")
    subject: Subject
    predicate: NamedNode
    object: Term
    graph_name: GraphName

    def __init__(
        self,
        subject: Subject,
        predicate: NamedNode,
        object: Term,  # noqa: A002
        graph_name: Optional[GraphName] = None,
    ) -> None:
        Triple(subject, predicate, object)  # validates the parts
        if graph_name is None:
            graph_name = DefaultGraph()
        elif not isinstance(graph_name, (NamedNode, BlankNode, DefaultGraph)):
            raise TypeError(f"a graph name is a NamedNode, BlankNode or DefaultGraph, not {type(graph_name).__name__}")
        _set(self, subject=subject, predicate=predicate, object=object, graph_name=graph_name)

    @property
    def triple(self) -> Triple:
        return Triple(self.subject, self.predicate, self.object)

    def __str__(self) -> str:
        t = f"{_inner(self.subject)} {self.predicate} {_inner(self.object)}"
        return t if isinstance(self.graph_name, DefaultGraph) else f"{t} {self.graph_name}"

    def __repr__(self) -> str:
        return (
            f"<Quad subject={self.subject!r} predicate={self.predicate!r} "
            f"object={self.object!r} graph_name={self.graph_name!r}>"
        )

    def __eq__(self, other: builtins.object) -> bool:
        return (
            type(other) is Quad
            and other.subject == self.subject
            and other.predicate == self.predicate
            and other.object == self.object
            and other.graph_name == self.graph_name
        )

    def __hash__(self) -> int:
        return hash((Quad, self.subject, self.predicate, self.object, self.graph_name))

    def __getitem__(self, index: int) -> Any:
        return (self.subject, self.predicate, self.object, self.graph_name)[index]

    def __len__(self) -> int:
        return 4

    def __iter__(self) -> Iterator[Any]:
        return iter((self.subject, self.predicate, self.object, self.graph_name))

    def __reduce__(self) -> Tuple[Any, ...]:
        return (Quad, (self.subject, self.predicate, self.object, self.graph_name))


def _unchecked_triple(s: Any, p: Any, o: Any) -> Triple:
    t = object.__new__(Triple)
    _set(t, subject=s, predicate=p, object=o)
    return t


def _unchecked_quad(s: Any, p: Any, o: Any, g: Any) -> Quad:
    q = object.__new__(Quad)
    _set(q, subject=s, predicate=p, object=o, graph_name=g)
    return q


# ------------------------------------------------------------------------- JSON conversion
#
# The forms of crates/oxilite-core/src/json.rs: RDF/JS-style objects with a "termType".

_DEFAULT_GRAPH_JSON = {"termType": "DefaultGraph", "value": ""}

AnyTerm = Union[NamedNode, BlankNode, Literal, DefaultGraph, Triple, Quad]


def to_json(t: AnyTerm) -> Dict[str, Any]:
    """The JSON form of a term, triple or quad."""
    if isinstance(t, NamedNode):
        return {"termType": "NamedNode", "value": t.value}
    if isinstance(t, BlankNode):
        return {"termType": "BlankNode", "value": t.value}
    if isinstance(t, Literal):
        return {
            "termType": "Literal",
            "value": t.value,
            "language": t.language or "",
            "direction": t.direction.value if t.direction else "",
            "datatype": {"termType": "NamedNode", "value": t.datatype.value},
        }
    if isinstance(t, DefaultGraph):
        return _DEFAULT_GRAPH_JSON
    if isinstance(t, Triple):
        return {
            "termType": "Quad",
            "value": "",
            "subject": to_json(t.subject),
            "predicate": to_json(t.predicate),
            "object": to_json(t.object),
            "graph": _DEFAULT_GRAPH_JSON,
        }
    if isinstance(t, Quad):
        return {
            "termType": "Quad",
            "value": "",
            "subject": to_json(t.subject),
            "predicate": to_json(t.predicate),
            "object": to_json(t.object),
            "graph": to_json(t.graph_name),
        }
    raise TypeError(f"not an RDF term: {t!r}")


def from_json(j: Dict[str, Any]) -> Any:
    """A term from its JSON form (a ``Quad`` JSON in a term position is a ``Triple``)."""
    kind = j["termType"]
    if kind == "NamedNode":
        return NamedNode._unchecked(j["value"])
    if kind == "BlankNode":
        return BlankNode._unchecked(j["value"])
    if kind == "Literal":
        return Literal._unchecked(j["value"], j["datatype"]["value"], j.get("language"), j.get("direction"))
    if kind == "DefaultGraph":
        return DefaultGraph()
    if kind == "Quad":
        return _unchecked_triple(from_json(j["subject"]), from_json(j["predicate"]), from_json(j["object"]))
    raise ValueError(f"unsupported termType {kind}")


def quad_from_json(j: Dict[str, Any]) -> Quad:
    g = j.get("graph")
    return _unchecked_quad(
        from_json(j["subject"]),
        from_json(j["predicate"]),
        from_json(j["object"]),
        from_json(g) if g else DefaultGraph(),
    )


def as_quad(q: Union[Quad, Triple]) -> Quad:
    """A quad from a quad or a triple (in the default graph)."""
    if isinstance(q, Quad):
        return q
    if isinstance(q, Triple):
        return _unchecked_quad(q.subject, q.predicate, q.object, DefaultGraph())
    raise TypeError(f"expected a Quad or a Triple, not {type(q).__name__}")

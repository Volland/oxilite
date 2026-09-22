---
lat:
  require-code-mention: true
---
# Tests

Test specifications that are implemented in code; each leaf is referenced by exactly one `@lat:` comment next to its test. Planned tests live in [[test-plan]].

## Encoding

Unit tests of the tagged 64-bit term encoding described in [[architecture#Term encoding]].

### Inline integers sort by value

Canonical integers in ±2^58 encode inline, ids are positive and ordered like the values, decode back exactly, and out-of-range values are not inlined.

### Non-canonical literals are hashed

`"012"^^xsd:integer` is a different term from `"12"^^xsd:integer`: it is hashed (Typed tag) with its numeric value 12 in the side columns, preserving term identity.

### Simple literal equals xsd string

A simple literal and the same lexical form typed `xsd:string` get the same id (RDF 1.1), while a language-tagged literal with that form gets a different one.

### Tags partition the id space

An IRI and a blank node with the same text get different ids, and each id falls in the range reserved for its tag, so kind checks are integer range checks.

## Write path

Unit tests of statement generation for inserts, see [[architecture#Write path]].

### Statements respect the size limit

Inserting 500 quads under a 2 000-byte statement limit yields several statements, none longer than the limit and none using bound parameters.

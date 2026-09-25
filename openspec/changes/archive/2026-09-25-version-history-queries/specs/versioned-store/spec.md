## ADDED Requirements

### Requirement: History as data
A SPARQL query SHALL be able to read the history from the graph `<oxilite:history>`. A commit is its tick, an
`xsd:integer` (the `n` of `#n`), described with PROV-O:
- `a prov:Activity`
- `prov:startedAtTime` (an `xsd:dateTime`)
- `prov:wasAssociatedWith` (the author, a string)
- `rdfs:comment` (the message)
- `prov:wasInformedBy` (the commit before)

Its changes are `?c oxl:added <<( s p o )>>` and `?c oxl:removed <<( s p o )>>` (`oxl:` is
`https://oxilite.dev/ns#`); the triple term's parts may be variables. Reading changes without a change log
SHALL fail; a query on the history graph SHALL never be answered by the fallback evaluator.

#### Scenario: Who removed a fact
- **WHEN** `SELECT ?who ?when { GRAPH <oxilite:history> { ?c oxl:removed <<( ex:alice ex:role ex:admin )>> ; prov:wasAssociatedWith ?who ; prov:startedAtTime ?when } }` runs
- **THEN** it returns the author and time of every commit that removed that triple

#### Scenario: Every change with its author
- **WHEN** `SELECT ?who ?s ?p ?o { GRAPH <oxilite:history> { ?c oxl:added <<( ?s ?p ?o )>> ; prov:wasAssociatedWith ?who } }` runs
- **THEN** it returns every added triple with the author of its commit

#### Scenario: Changes without a log
- **WHEN** a query reads `oxl:added` on a store at level `stamped`
- **THEN** it fails with an error stating that the store keeps no change log

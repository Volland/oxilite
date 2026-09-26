## ADDED Requirements

### Requirement: Schema registry commands
The shell SHALL provide the following commands. GRAPH is `<iri>`, a prefixed name, a bare IRI,
or `DEFAULT`.
- `.register ROLE GRAPH ?FILE?` registers GRAPH with a role.
- `.map GRAPH TARGET…` sets the graphs a registration applies to; `ALL` means every graph.
- `.unregister GRAPH ?--drop?` removes the registration, and with `--drop` the graph's triples.
- `.activate GRAPH` and `.deactivate GRAPH` switch a registration on or off.
- `.registry` lists the registrations with their mapping.
- `.shapes` lists the shape index.

#### Scenario: Register, map and list
- **WHEN** `.register ontology ex:onto onto.ttl` then `.map ex:onto ex:data` are entered
- **THEN** `.registry` shows `ex:onto` as an active ontology applying to `ex:data`

### Requirement: Session query options
The shell SHALL apply `.reasoning ?none|rdfs|owl-ql?`, `.inferred ?on|off?` and
`.schemagraphs ?on|off?` to every query and `.explain`, and keep default options for its own
reads (`.dump`, `.stats`, `.graphs`, completion). `.materialize ?clear?` SHALL run or clear
OWL 2 RL materialization.

#### Scenario: Reasoning in the shell
- **WHEN** `ex:Dog rdfs:subClassOf ex:Animal` and `ex:rex a ex:Dog` are stored and `.reasoning rdfs` is set
- **THEN** `SELECT ?x { ?x a ex:Animal }` returns `ex:rex`, and `.dump` writes no `ex:rex a ex:Animal`

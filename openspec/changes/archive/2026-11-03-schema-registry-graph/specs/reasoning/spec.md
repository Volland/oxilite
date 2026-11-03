## ADDED Requirements

### Requirement: Scoped TBox closure
The system SHALL keep one TBox closure per scope: one for all graphs, and one for each graph
that some active ontology applies to specifically. It SHALL rewrite each pattern so that a quad
of graph G is entailed with G's closure, or with the all-graphs closure when G has none of its
own. With no specific mapping, the rewriting SHALL compare the scope to a constant.

#### Scenario: Unmapped store is unchanged
- **WHEN** no ontology applies to a specific graph
- **THEN** every query returns what it returned before scopes existed

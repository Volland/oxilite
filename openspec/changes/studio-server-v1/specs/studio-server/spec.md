## ADDED Requirements

### Requirement: Live SHACL diagnostics on source lines
The server SHALL validate the Project store in the background after every change and publish
each result as a diagnostic on the source line of the focus node's statement.

#### Scenario: Entailed type violates a shape
- **WHEN** RDFS reasoning makes an employee a person and the person shape requires a name
- **THEN** a diagnostic appears on the employee's line in its data file

### Requirement: Justifications
`oxilite/why` SHALL return a proof tree for an entailed triple, down to asserted statements
with their source lines.

#### Scenario: Rule conclusion
- **WHEN** a rule file derives a triple
- **THEN** the tree names the rule file and its rule, with the premises it matched

### Requirement: Project checks without the editor
`oxilite check` SHALL report load errors, SHACL results and manifest test outcomes, and exit
with status 1 when any fails.

#### Scenario: Failing test
- **WHEN** a manifest test's expected results differ from the actual ones
- **THEN** the check reports what is missing and fails

## Purpose

Provides reproducible performance measurements of oxilite against Oxigraph, so query-speed claims and planner changes are backed by data.

## ADDED Requirements

### Requirement: Reproducible benchmark
The project SHALL provide scripts that generate BSBM datasets at fixed scales, load them into oxilite and into Oxigraph, run the explore and business-intelligence query mixes, and record results in a machine-readable format.

#### Scenario: Re-run gives comparable numbers
- **WHEN** the benchmark script runs twice on the same machine
- **THEN** both runs produce result files with the same schema, and query-mix timings for each engine

### Requirement: Published comparison
The project SHALL publish a comparison table for each release: queries per second per mix, load time and database size, for oxilite backends and Oxigraph.

#### Scenario: README table
- **WHEN** a release is tagged
- **THEN** the README contains the benchmark table for that release

### Requirement: Write-cost report
The project SHALL report, for D1, the number of rows written per inserted triple, including index entries, for each schema configuration.

#### Scenario: Graph index cost
- **WHEN** the report is generated for stores with and without the graph index
- **THEN** it shows the difference in rows written per triple

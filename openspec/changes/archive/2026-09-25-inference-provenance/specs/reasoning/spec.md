## ADDED Requirements

### Requirement: Inferences are attributed to producers
The system SHALL record which producer (`owl2rl`, or a Datalog program's producer name)
derived each materialized inference, and materializing one producer SHALL replace only its
own conclusions.

#### Scenario: Rules and OWL 2 RL side by side
- **WHEN** OWL 2 RL is materialized, then a rule program under producer `rules/a.dl`
- **THEN** both producers' conclusions are present and each names its producer

#### Scenario: Clearing one producer
- **WHEN** `clear_inferences_of("rules/a.dl")` runs
- **THEN** the rule conclusions are gone and the OWL 2 RL conclusions remain

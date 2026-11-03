//! Notation3 rules over oxilite, run by the [eyeron](https://crates.io/crates/eyeron) reasoner.
//!
//! A rule set is plain N3 (`{ ?x :parent ?y . ?y :parent ?z } => { ?x :grandparent ?z } .`).
//! A run reads one scope of the store (every graph but the system graphs, or chosen graphs),
//! and inside it only the predicates the rule bodies use; eyeron reasons in memory. Then:
//!
//! - [`ReasonJob`] returns what follows, writing nothing;
//! - [`MaterializeJob`] stores the conclusions in `quads_inf` under a producer name (`n3` by
//!   default), replacing that producer's earlier conclusions, so queries see them with
//!   `include_inferred` exactly as they see OWL 2 RL and Datalog inferences.
//!
//! Both are sans-IO jobs, so `Store` and `AsyncStore` (Cloudflare D1 included) run them alike.
//!
// @lat: [[architecture#N3 reasoning]]

mod error;
mod rules;
mod run;
mod term;

pub use error::{N3Error, Result};
pub use rules::{is_builtin, Monotonicity, RuleSet, RuleSetInfo};
pub use run::{
    reason_over, Limits, MaterializeJob, MaterializeStats, N3Result, Options, ReasonJob,
    N3_PRODUCER,
};
pub use term::{from_n3, to_n3, triple_from_n3, triple_to_n3};

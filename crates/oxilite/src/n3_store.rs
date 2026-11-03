//! N3 reasoning on the blocking and async stores (`n3_reason`, `n3_materialize`).
//!
// @lat: [[architecture#N3 reasoning]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::{AsyncBackend, SyncBackend};
use oxilite_n3::{MaterializeJob, MaterializeStats, N3Error, N3Result, Options, ReasonJob};

/// A store error that is really a reasoning outcome (an incomplete run, a fired fuse) keeps its
/// own variant.
fn n3_error(e: oxilite_core::Error) -> N3Error {
    match e {
        oxilite_core::Error::Other(m) if m.starts_with("N3 reasoning is incomplete: ") => {
            N3Error::Incomplete(m["N3 reasoning is incomplete: ".len()..].to_owned())
        }
        oxilite_core::Error::Other(m) if m.starts_with("N3 inference fuse fired: ") => {
            N3Error::Fuse(m["N3 inference fuse fired: ".len()..].to_owned())
        }
        e => N3Error::Store(e),
    }
}

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// Runs N3 rules over the store and returns what follows, without writing.
    ///
    /// ```
    /// # use oxilite::store::Store;
    /// let store = Store::new()?;
    /// store.update(
    ///     "PREFIX ex: <http://example.org/>
    ///      INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
    /// )?;
    /// let r = store.n3_reason(
    ///     "@prefix ex: <http://example.org/> .
    ///      { ?x ex:parent ?y . ?y ex:parent ?z } => { ?x ex:grandparent ?z } .",
    /// )?;
    /// assert_eq!(r.derived.len(), 1);
    /// # Result::<_, Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn n3_reason(&self, rules: &str) -> Result<N3Result, N3Error> {
        self.n3_reason_with(rules, &Options::default())
    }

    /// Runs N3 rules with options (scope, inferences, pinned clock, limits, proof).
    pub fn n3_reason_with(&self, rules: &str, options: &Options) -> Result<N3Result, N3Error> {
        let job = ReasonJob::parse(rules, options.clone(), self.caps())?;
        self.run(job).map_err(n3_error)
    }

    /// Stores what N3 rules conclude as inferences of the producer `n3`, replacing its earlier
    /// ones in one atomic request. Queries see them with `include_inferred`.
    pub fn n3_materialize(&self, rules: &str) -> Result<MaterializeStats, N3Error> {
        self.n3_materialize_with(rules, &Options::default())
    }

    /// Materializes with options; `options.producer` names the rule set's inferences.
    pub fn n3_materialize_with(
        &self,
        rules: &str,
        options: &Options,
    ) -> Result<MaterializeStats, N3Error> {
        let job = MaterializeJob::parse(rules, options.clone(), self.caps())?;
        self.run(job).map_err(n3_error)
    }
}

impl<B: AsyncBackend> AsyncStore<B> {
    /// Runs N3 rules without writing (see [`Store::n3_reason`]).
    // @tg: implements:: [[openspec:n3-rules#Every backend]] {since: "2026-11-03"}
    pub async fn n3_reason(&self, rules: &str) -> Result<N3Result, N3Error> {
        self.n3_reason_with(rules, &Options::default()).await
    }

    /// Runs N3 rules with options.
    pub async fn n3_reason_with(
        &self,
        rules: &str,
        options: &Options,
    ) -> Result<N3Result, N3Error> {
        let job = ReasonJob::parse(rules, options.clone(), self.caps())?;
        oxilite_core::run_async(&self.backend, job)
            .await
            .map_err(n3_error)
    }

    /// Stores what N3 rules conclude (see [`Store::n3_materialize`]).
    pub async fn n3_materialize(&self, rules: &str) -> Result<MaterializeStats, N3Error> {
        self.n3_materialize_with(rules, &Options::default()).await
    }

    /// Materializes with options.
    pub async fn n3_materialize_with(
        &self,
        rules: &str,
        options: &Options,
    ) -> Result<MaterializeStats, N3Error> {
        let job = MaterializeJob::parse(rules, options.clone(), self.caps())?;
        oxilite_core::run_async(&self.backend, job)
            .await
            .map_err(n3_error)
    }
}

//! Versioning on the blocking store: levels, commit metadata, as-of queries and history.
//!
// @lat: [[architecture#Versioning]]

use crate::store::Store;
use oxilite_core::job::{run_async, run_sync};
use oxilite_core::version::{self, Change, CommitRecord, LevelChange, VersionStatus, Versioning};
use oxilite_core::{CommitInfo, QueryOptions, Result, SyncBackend};
use oxrdf::{GraphNameRef, NamedNodeRef, NamedOrBlankNodeRef, TermRef};
use std::borrow::Cow;

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// The versioning level and where the clock and history stand.
    pub fn versioning(&self) -> Result<VersionStatus> {
        self.run(version::status_job(self.stats().version))
    }

    /// Changes the versioning level (and its index options). Upgrades never lose data; a
    /// downgrade keeps the recorded history (frozen) and the ticks unless
    /// `change.allow_loss`. One atomic request; returns the new status.
    pub fn set_versioning(&self, level: Versioning, change: LevelChange) -> Result<VersionStatus> {
        let stats = self.stats();
        let job = version::level_change_job(
            &stats.version,
            level,
            &change,
            self.backend().capabilities(),
        )?;
        // The level change must not open a write tick of its own: run it on the raw backend.
        let stats = run_sync(self.backend(), job)?;
        self.set_stats(stats);
        self.versioning()
    }

    /// Author and message recorded on the ticks of the following writes (until changed).
    pub fn set_commit_info(&self, info: CommitInfo) {
        self.versioned().set_commit_info(info);
    }

    /// Author and message recorded on the ticks of writes.
    pub fn commit_info(&self) -> CommitInfo {
        self.versioned().commit_info()
    }

    /// Runs `f` with `info` recorded on its writes, then restores the previous commit info.
    pub fn with_commit<T>(
        &self,
        info: CommitInfo,
        f: impl FnOnce(&Self) -> Result<T>,
    ) -> Result<T> {
        let previous = self.commit_info();
        self.set_commit_info(info);
        let out = f(self);
        self.set_commit_info(previous);
        out
    }

    /// The tick a version reference (`HEAD~2`, `#42`, `@2026-09-01T00:00:00Z`) designates.
    pub fn resolve_version(&self, version: &str) -> Result<i64> {
        let r = version.parse()?;
        let ticks = self.run(version::resolve_job(vec![r], self.stats().version))?;
        Ok(ticks[0])
    }

    /// The latest `limit` commits and level changes, newest first.
    pub fn history(&self, limit: usize) -> Result<Vec<CommitRecord>> {
        self.run(version::log_job(self.stats().version, limit)?)
    }

    /// The changes after tick `after`, up to tick `until` (inclusive). With a change log, every
    /// addition and removal; with only the clock, the current quads added in that range.
    pub fn changes(&self, after: i64, until: Option<i64>) -> Result<Vec<Change>> {
        self.run(version::changes_job(
            self.stats().version,
            after,
            until,
            self.caps(),
        )?)
    }

    /// The net difference between two versions: quads added and removed from `from` to `to`.
    pub fn diff(&self, from: &str, to: &str) -> Result<Vec<Change>> {
        let state = self.stats().version;
        let ticks = self.run(version::resolve_job(
            vec![from.parse()?, to.parse()?],
            state,
        ))?;
        self.run(version::diff_job(state, ticks[0], ticks[1], self.caps())?)
    }

    /// Removes the quads matching the pattern from the store *and from the whole history*,
    /// recording a purge tick that names no removed content — for erasure requests. `None`
    /// matches anything; with every part `None` the whole store and history are purged.
    pub fn purge(
        &self,
        subject: Option<NamedOrBlankNodeRef<'_>>,
        predicate: Option<NamedNodeRef<'_>>,
        object: Option<TermRef<'_>>,
        graph_name: Option<GraphNameRef<'_>>,
        reason: Option<&str>,
    ) -> Result<()> {
        use oxilite_core::encoding::{graph_id, named_node_id, subject_id, term_id};
        let pattern = [
            subject.map(subject_id),
            predicate.map(|p| named_node_id(p.as_str())),
            object.map(term_id),
            graph_name.map(graph_id),
        ];
        let info = self.commit_info();
        let request = version::purge_request(
            self.stats().version,
            pattern,
            info.author.as_deref(),
            reason,
        );
        self.versioned().execute(&request)?;
        Ok(())
    }

    /// The options with every version the query names resolved to a tick.
    pub(crate) fn resolve_versions<'o>(
        &self,
        query: &spargebra::Query,
        options: &'o QueryOptions,
    ) -> Result<Cow<'o, QueryOptions>> {
        let refs = version::query_version_refs(query, options);
        if refs.is_empty() {
            return Ok(Cow::Borrowed(options));
        }
        let job = version::resolve_query_job(refs, self.stats().version, options.clone())?;
        Ok(Cow::Owned(self.run(job)?))
    }
}

impl<B: oxilite_core::AsyncBackend> crate::AsyncStore<B> {
    /// The versioning level and where the clock and history stand.
    pub async fn versioning(&self) -> Result<VersionStatus> {
        let state = self.stats.borrow().version;
        run_async(&self.backend, version::status_job(state)).await
    }

    /// Changes the versioning level; see [`Store::set_versioning`].
    pub async fn set_versioning(
        &self,
        level: Versioning,
        change: LevelChange,
    ) -> Result<VersionStatus> {
        let state = self.stats.borrow().version;
        let job = version::level_change_job(&state, level, &change, self.backend().capabilities())?;
        let stats = run_async(self.backend(), job).await?;
        self.set_stats(stats);
        self.versioning().await
    }

    /// Author and message recorded on the ticks of the following writes (until changed).
    pub fn set_commit_info(&self, info: CommitInfo) {
        self.backend.set_commit_info(info);
    }

    /// The tick a version reference designates.
    pub async fn resolve_version(&self, version: &str) -> Result<i64> {
        let r = version.parse()?;
        let state = self.stats.borrow().version;
        Ok(run_async(&self.backend, version::resolve_job(vec![r], state)).await?[0])
    }

    /// The latest `limit` commits and level changes, newest first.
    pub async fn history(&self, limit: usize) -> Result<Vec<CommitRecord>> {
        let state = self.stats.borrow().version;
        run_async(&self.backend, version::log_job(state, limit)?).await
    }

    /// The changes after tick `after`, up to tick `until`; see [`Store::changes`].
    pub async fn changes(&self, after: i64, until: Option<i64>) -> Result<Vec<Change>> {
        let state = self.stats.borrow().version;
        run_async(
            &self.backend,
            version::changes_job(state, after, until, self.caps())?,
        )
        .await
    }

    /// The net difference between two versions.
    pub async fn diff(&self, from: &str, to: &str) -> Result<Vec<Change>> {
        let state = self.stats.borrow().version;
        let ticks = run_async(
            &self.backend,
            version::resolve_job(vec![from.parse()?, to.parse()?], state),
        )
        .await?;
        run_async(
            &self.backend,
            version::diff_job(state, ticks[0], ticks[1], self.caps())?,
        )
        .await
    }

    /// Removes the quads matching the pattern from the store and the whole history; see
    /// [`Store::purge`].
    pub async fn purge(
        &self,
        subject: Option<NamedOrBlankNodeRef<'_>>,
        predicate: Option<NamedNodeRef<'_>>,
        object: Option<TermRef<'_>>,
        graph_name: Option<GraphNameRef<'_>>,
        reason: Option<&str>,
    ) -> Result<()> {
        use oxilite_core::encoding::{graph_id, named_node_id, subject_id, term_id};
        let pattern = [
            subject.map(subject_id),
            predicate.map(|p| named_node_id(p.as_str())),
            object.map(term_id),
            graph_name.map(graph_id),
        ];
        let state = self.stats.borrow().version;
        let info = self.backend.commit_info();
        let request = version::purge_request(state, pattern, info.author.as_deref(), reason);
        oxilite_core::AsyncBackend::execute(&self.backend, &request).await?;
        Ok(())
    }

    pub(crate) async fn resolve_versions<'o>(
        &self,
        query: &spargebra::Query,
        options: &'o QueryOptions,
    ) -> Result<Cow<'o, QueryOptions>> {
        let refs = version::query_version_refs(query, options);
        if refs.is_empty() {
            return Ok(Cow::Borrowed(options));
        }
        let state = self.stats.borrow().version;
        let job = version::resolve_query_job(refs, state, options.clone())?;
        Ok(Cow::Owned(run_async(&self.backend, job).await?))
    }
}

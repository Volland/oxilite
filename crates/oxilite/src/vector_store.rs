//! Vector indexes and host functions on the blocking store.
//!
// @lat: [[architecture#Vector indexes]]

use crate::store::Store;
use oxilite_core::functions::{FunctionRegistry, Functions, HostFunction};
use oxilite_core::vector::{self, QueryVector, VectorIndex};
use oxilite_core::writer::EncodedQuads;
use oxilite_core::{Error, Request, Result, SqlValue, Statement, SyncBackend};
use oxrdf::{Quad, Term};
use std::sync::Arc;

/// A vector index and its state.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorIndexInfo {
    pub index: VectorIndex,
    /// The table matches the definition.
    pub built: bool,
    /// Embeddings indexed (0 when not built).
    pub rows: u64,
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorHit {
    pub node: Term,
    /// The metric's distance: lower is nearer.
    pub distance: f64,
    /// The similarity score (see `Metric::score`): higher is nearer.
    pub score: f64,
}

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    // ------------------------------------------------------------------ host functions

    /// Registers a host function, callable from SPARQL by its IRI, from Cypher by its Cypher
    /// name and from Datalog by its IRI (see `oxilite_core::functions`). It applies to this
    /// handle and its clones, and is not stored in the database.
    pub fn register_function(&self, function: HostFunction) -> Result<()> {
        #[cfg(feature = "cypher")]
        if oxilite_cypher::is_builtin_function(function.cypher()) {
            return Err(Error::Other(format!(
                "Cypher name {} is a built-in function",
                function.cypher()
            )));
        }
        let mut guard = self
            .inner_functions()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut registry = guard.registry().cloned().unwrap_or_default();
        registry.register(function)?;
        *guard = Functions(Some(Arc::new(registry)));
        Ok(())
    }

    /// Removes a host function; `false` when none has this IRI.
    pub fn unregister_function(&self, iri: &str) -> bool {
        let mut guard = self
            .inner_functions()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut registry: FunctionRegistry = guard.registry().cloned().unwrap_or_default();
        let removed = registry.unregister(iri);
        *guard = Functions(Some(Arc::new(registry)));
        removed
    }

    /// The registered host functions.
    pub fn functions(&self) -> Vec<HostFunction> {
        self.host_functions()
            .registry()
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The registry, as the frontends' options carry it.
    pub fn host_functions(&self) -> Functions {
        self.inner_functions()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    // ------------------------------------------------------------------ vector indexes

    fn require_vectors(&self) -> Result<()> {
        if self.caps().vectors {
            Ok(())
        } else {
            Err(Error::unsupported(format!(
                "vector indexes need a backend with vector functions, such as Turso (Store::open_turso); {} has none",
                self.caps().name
            )))
        }
    }

    /// Creates a vector index: its definition in `<oxilite:vectors>`, its table and triggers,
    /// back-filled from the existing embeddings, in one atomic request. A malformed existing
    /// value fails the creation and changes nothing.
    pub fn create_vector_index(&self, index: &VectorIndex) -> Result<()> {
        self.require_vectors()?;
        index.validate()?;
        let stats = self.stats();
        if let Some(other) = stats
            .vector_indexes
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(&index.name))
        {
            return Err(Error::Other(format!(
                "a vector index named {} already exists",
                other.name
            )));
        }
        self.check_embeddings(index)?;
        let quads = index.to_quads();
        let mut statements =
            EncodedQuads::new(quads.iter().map(Quad::as_ref)).insert_statements(self.caps());
        // A table left by an earlier definition of the same name goes first.
        if stats.vector_built.contains_key(&index.name.to_lowercase()) {
            statements.extend(vector::drop_statements(&index.name));
        }
        statements.extend(index.create_statements(self.caps())?);
        self.versioned().execute(&Request::atomic(statements))?;
        self.reload_stats()
    }

    /// Reports the first value of the indexed property that is not a vector of the index's
    /// dimensions, naming its subject (the atomic creation would only say that one exists).
    fn check_embeddings(&self, index: &VectorIndex) -> Result<()> {
        let pid = oxilite_core::encoding::named_node_id(index.property.as_str());
        let rows = self.backend().execute(&Request::read(vec![Statement::new(format!(
            "SELECT s.lex, t.lex FROM quads q LEFT JOIN terms t ON t.id = q.o LEFT JOIN terms s ON s.id = q.s \
             WHERE q.p = {pid}"
        ))]))?;
        for row in rows.into_iter().next().map(|r| r.rows).unwrap_or_default() {
            let subject = row.first().cloned().and_then(SqlValue::into_string);
            let value = row.get(1).cloned().and_then(SqlValue::into_string);
            let ok = value
                .as_deref()
                .and_then(vector::parse_vector)
                .is_some_and(|v| v.len() == index.dimensions as usize);
            if !ok {
                return Err(Error::Other(format!(
                    "cannot create vector index {}: {} has {} {}, which is not a JSON array of {} numbers",
                    index.name,
                    subject.map_or_else(|| "a node".to_owned(), |s| format!("<{s}>")),
                    oxilite_core::functions::local_name(index.property.as_str()),
                    value.map_or_else(|| "a non-literal value".to_owned(), |v| format!("{v:?}")),
                    index.dimensions
                )));
            }
        }
        Ok(())
    }

    /// Drops a vector index (its definition and its table); `false` when none has this name.
    pub fn drop_vector_index(&self, name: &str) -> Result<bool> {
        let stats = self.stats();
        let Some(index) = vector::find(&stats.vector_indexes, name).cloned() else {
            return Ok(false);
        };
        self.require_vectors()?;
        let mut statements = vec![Statement::new(format!(
            "DELETE FROM quads WHERE g = {} AND s = {}",
            oxilite_core::encoding::named_node_id(vector::VECTORS_GRAPH),
            oxilite_core::encoding::named_node_id(index.iri().as_str())
        ))];
        statements.extend(vector::drop_statements(&index.name));
        self.versioned().execute(&Request::atomic(statements))?;
        self.reload_stats()?;
        Ok(true)
    }

    /// The vector indexes, with whether each is built and how many embeddings it holds.
    pub fn vector_indexes(&self) -> Result<Vec<VectorIndexInfo>> {
        // Definitions may have arrived through any write path (a load, an insert).
        self.reload_stats()?;
        let stats = self.stats();
        let mut out = Vec::new();
        for index in &stats.vector_indexes {
            let built = vector::is_built(index, &stats.vector_built);
            let rows = if built {
                self.backend()
                    .execute(&Request::read(vec![Statement::new(format!(
                        "SELECT COUNT(*) FROM {}",
                        index.table()
                    ))]))?
                    .first()
                    .and_then(|r| r.rows.first())
                    .and_then(|r| r.first())
                    .and_then(SqlValue::as_i64)
                    .unwrap_or(0) as u64
            } else {
                0
            };
            out.push(VectorIndexInfo {
                index: index.clone(),
                built,
                rows,
            });
        }
        Ok(out)
    }

    /// Why descriptions in `<oxilite:vectors>` are not valid index definitions.
    pub fn vector_index_problems(&self) -> Vec<String> {
        self.stats().vector_problems
    }

    /// Makes the built indexes match the definitions in `<oxilite:vectors>` (after a load that
    /// brought definitions, or a SPARQL update of them — updates call this themselves).
    /// Returns whether anything changed.
    pub fn sync_vector_indexes(&self) -> Result<bool> {
        if !self.caps().vectors {
            return Ok(false);
        }
        self.reload_stats()?;
        let stats = self.stats();
        let statements =
            vector::sync_statements(&stats.vector_indexes, &stats.vector_built, self.caps())?;
        if statements.is_empty() {
            return Ok(false);
        }
        self.versioned().execute(&Request::atomic(statements))?;
        self.reload_stats()?;
        Ok(true)
    }

    pub(crate) fn sync_vector_indexes_if_needed(&self) -> Result<bool> {
        let stats = self.stats();
        if !self.caps().vectors
            || (stats.vector_indexes.is_empty() && stats.vector_built.is_empty())
        {
            return Ok(false);
        }
        self.sync_vector_indexes()
    }

    /// The `k` nodes nearest to `query` in an index, nearest first.
    ///
    /// ```
    /// # #[cfg(feature = "turso")] {
    /// use oxilite::store::Store;
    /// use oxilite::vector::{QueryVector, VectorIndex};
    /// use oxilite::model::NamedNode;
    ///
    /// let store = Store::new_turso()?;
    /// store.update(r#"PREFIX ex: <http://example.com/>
    ///     INSERT DATA { ex:a ex:e "[1, 0]" . ex:b ex:e "[0, 1]" }"#)?;
    /// store.create_vector_index(&VectorIndex::new("docs", NamedNode::new("http://example.com/e")?, 2))?;
    /// let hits = store.vector_search("docs", &QueryVector::vector(&[0.9, 0.1]), 1)?;
    /// assert_eq!(hits[0].node.to_string(), "<http://example.com/a>");
    /// # }
    /// # Result::<_, Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn vector_search(&self, name: &str, query: &QueryVector, k: u64) -> Result<Vec<VectorHit>> {
        self.require_vectors()?;
        let stats = self.stats();
        let index = vector::find(&stats.vector_indexes, name)
            .ok_or_else(|| Error::Other(format!("there is no vector index named {name}")))?;
        if !vector::is_built(index, &stats.vector_built) {
            return Err(Error::Other(format!(
                "vector index {name} is defined but not built; run sync_vector_indexes"
            )));
        }
        let sql = index.knn_sql(query, k)?;
        let rows = self
            .backend()
            .execute(&Request::read(vec![Statement::new(format!(
                "SELECT k.s, k.d, t.lex, t.dt, t.lang, t.dir FROM ({sql}) AS k LEFT JOIN terms t ON t.id = k.s ORDER BY k.d, k.s"
            ))]))?;
        let mut out = Vec::new();
        for row in rows.into_iter().next().map(|r| r.rows).unwrap_or_default() {
            let get = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
            let (Some(id), Some(d)) = (get(0).as_i64(), get(1).as_f64()) else {
                continue;
            };
            let node = match oxilite_core::encoding::decode_inline(id) {
                Some(t) => t,
                None => oxilite_core::encoding::decode_row(
                    id,
                    get(2).into_string().unwrap_or_default(),
                    get(3).into_string(),
                    get(4).into_string(),
                    get(5).as_i64(),
                )?,
            };
            out.push(VectorHit {
                node,
                distance: d,
                score: index.metric.score(d),
            });
        }
        Ok(out)
    }
}

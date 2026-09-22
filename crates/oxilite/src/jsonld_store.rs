//! JSON-LD documents on the blocking and async stores (`Store::jsonld`).
//!
// @lat: [[architecture#JSON-LD documents]]

use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::job::{run_async, run_sync, Job};
use oxilite_core::{AsyncBackend, SyncBackend};
use oxilite_jsonld::{
    document_for_graph_job, document_graphs_job, find_job, from_core, get_job, list_contexts_job,
    list_job, put_context_request, remove_context_request, schema, CheckJob, DocumentFilter,
    DocumentInput, Drift, JsonLdError, JsonLdOptions, Loader, NoContexts, RebuildJob,
    StoredDocument, WriteJob, WriteOutcome,
};
use oxrdf::{GraphName, GraphNameRef};

type R<T> = Result<T, JsonLdError>;

/// JSON-LD documents stored in a [`Store`]: each document verbatim in `jsonld_documents`,
/// its RDF in a named graph (by default the document's `id`).
///
/// ```
/// use oxilite::store::Store;
///
/// let store = Store::new()?;
/// let docs = store.jsonld()?;
/// let key = docs.put_document(r#"{
///     "@context": {"name": "http://schema.org/name"},
///     "@id": "https://example.org/people/ada",
///     "name": "Ada Lovelace"
/// }"#)?;
/// assert_eq!(key, "https://example.org/people/ada");
/// // The RDF is in the graph named after the key.
/// let n = store.query("ASK { GRAPH <https://example.org/people/ada> { ?s <http://schema.org/name> \"Ada Lovelace\" } }")?;
/// assert!(matches!(n, oxilite::sparql::QueryResults::Boolean(true)));
/// // The JSON comes back byte for byte.
/// assert!(docs.get_document(&key)?.unwrap().json.contains("Ada Lovelace"));
/// # Result::<_, Box<dyn std::error::Error>>::Ok(())
/// ```
pub struct JsonLdStore<'a, B: SyncBackend, S = NoContexts> {
    store: &'a Store<B>,
    options: JsonLdOptions,
    first: S,
}

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// JSON-LD documents with the default options (key = `@id`/`id`, graph = key).
    /// Creates the JSON-LD tables on first use.
    pub fn jsonld(&self) -> R<JsonLdStore<'_, B>> {
        self.jsonld_with(JsonLdOptions::default())
    }

    /// JSON-LD documents with explicit options.
    pub fn jsonld_with(&self, options: JsonLdOptions) -> R<JsonLdStore<'_, B>> {
        self.jsonld_with_loader(options, NoContexts)
    }

    /// JSON-LD documents whose unknown contexts are first looked up in `first` (for example
    /// bundled contexts), then in the persisted `jsonld_contexts` table.
    pub fn jsonld_with_loader<S: Loader>(
        &self,
        options: JsonLdOptions,
        first: S,
    ) -> R<JsonLdStore<'_, B, S>> {
        self.backend()
            .execute(&schema::create_schema(&options.indexes))
            .map_err(from_core)?;
        Ok(JsonLdStore {
            store: self,
            options,
            first,
        })
    }
}

impl<B: SyncBackend + Send + Sync + 'static, S: Loader> JsonLdStore<'_, B, S> {
    /// The options of this handle.
    pub fn options(&self) -> &JsonLdOptions {
        &self.options
    }

    /// The store.
    pub fn store(&self) -> &Store<B> {
        self.store
    }

    fn run<J: Job>(&self, job: J) -> R<J::Output> {
        run_sync(self.store.backend(), job).map_err(from_core)
    }

    /// Runs a write; reads of previous versions and the write share one transaction
    /// when the backend has interactive transactions.
    fn write(&self, puts: Vec<DocumentInput>, removes: Vec<String>) -> R<WriteOutcome> {
        let job = WriteJob::new(
            puts,
            removes,
            self.options.clone(),
            &self.first,
            self.store.caps().clone(),
        )?;
        let backend = self.store.backend();
        let tx =
            !self.options.graph.owns_target() && backend.capabilities().interactive_transactions;
        if tx {
            backend.begin().map_err(from_core)?;
        }
        match self.run(job) {
            Ok(o) => {
                if tx {
                    backend.commit().map_err(from_core)?;
                }
                Ok(o)
            }
            Err(e) => {
                if tx {
                    let _ = backend.rollback();
                }
                Err(e)
            }
        }
    }

    /// Stores a document (replacing any document with the same key) and returns its key.
    pub fn put_document(&self, json: &str) -> R<String> {
        self.put_documents(vec![DocumentInput::new(json)])
            .map(|mut o| o.keys.remove(0))
    }

    /// Stores a document under an explicit key.
    pub fn put_document_with_key(&self, key: &str, json: &str) -> R<String> {
        let mut input = DocumentInput::new(json);
        input.key = Some(key.to_owned());
        self.put_documents(vec![input])
            .map(|mut o| o.keys.remove(0))
    }

    /// Stores several documents in one atomic request.
    pub fn put_documents(&self, inputs: Vec<DocumentInput>) -> R<WriteOutcome> {
        self.write(inputs, Vec::new())
    }

    /// The stored document, byte for byte.
    pub fn get_document(&self, key: &str) -> R<Option<StoredDocument>> {
        Ok(self.run(get_job(key))?.into_iter().next())
    }

    /// Removes a document and every graph it owns; `false` when it was not stored.
    pub fn remove_document(&self, key: &str) -> R<bool> {
        Ok(self
            .write(Vec::new(), vec![key.to_owned()])?
            .removed
            .first()
            .copied()
            .unwrap_or(false))
    }

    /// Documents ordered by key, after `after` (keyset paging).
    pub fn list_documents(&self, after: Option<&str>, limit: usize) -> R<Vec<StoredDocument>> {
        self.run(list_job(after, limit))
    }

    /// Documents matching metadata filters (issuer, subject, type, validity, profile).
    pub fn find_documents(&self, filter: &DocumentFilter) -> R<Vec<StoredDocument>> {
        self.run(find_job(filter))
    }

    /// The graphs a document owns.
    pub fn document_graphs(&self, key: &str) -> R<Vec<GraphName>> {
        self.run(document_graphs_job(key))
    }

    /// The document behind a graph (e.g. a `?g` bound by SPARQL).
    pub fn document_for_graph<'b>(
        &self,
        graph: impl Into<GraphNameRef<'b>>,
    ) -> R<Option<StoredDocument>> {
        Ok(self
            .run(document_for_graph_job(graph.into()))?
            .into_iter()
            .next())
    }

    /// Persists a context, so documents referencing `iri` convert offline everywhere.
    pub fn put_context(&self, iri: &str, json: &str) -> R<()> {
        self.store
            .backend()
            .execute(&put_context_request(iri, json)?)
            .map_err(from_core)?;
        Ok(())
    }

    /// Removes a persisted context.
    pub fn remove_context(&self, iri: &str) -> R<()> {
        self.store
            .backend()
            .execute(&remove_context_request(iri))
            .map_err(from_core)?;
        Ok(())
    }

    /// IRIs of the persisted contexts.
    pub fn contexts(&self) -> R<Vec<String>> {
        self.run(list_contexts_job())
    }

    /// Regenerates a document's graphs from its stored JSON; `false` when it is not stored.
    pub fn rebuild_graph(&self, key: &str) -> R<bool> {
        let job = RebuildJob::new(
            key,
            self.options.clone(),
            &self.first,
            self.store.caps().clone(),
        );
        Ok(self.run(job)?.is_some())
    }

    /// Documents whose graphs differ from a fresh conversion of their JSON (for example
    /// after a SPARQL UPDATE edited them).
    pub fn check_documents(&self) -> R<Vec<Drift>> {
        self.run(CheckJob::new(self.options.clone(), &self.first))
    }
}

/// [`JsonLdStore`] over an [`AsyncStore`] (Cloudflare D1).
pub struct AsyncJsonLdStore<'a, B: AsyncBackend, S = NoContexts> {
    store: &'a AsyncStore<B>,
    options: JsonLdOptions,
    first: S,
}

impl<B: AsyncBackend> AsyncStore<B> {
    /// JSON-LD documents with the default options. Creates the JSON-LD tables on first use.
    pub async fn jsonld(&self) -> R<AsyncJsonLdStore<'_, B>> {
        self.jsonld_with(JsonLdOptions::default()).await
    }

    /// JSON-LD documents with explicit options.
    pub async fn jsonld_with(&self, options: JsonLdOptions) -> R<AsyncJsonLdStore<'_, B>> {
        self.jsonld_with_loader(options, NoContexts).await
    }

    /// JSON-LD documents with a first context loader (e.g. bundled contexts).
    pub async fn jsonld_with_loader<S: Loader>(
        &self,
        options: JsonLdOptions,
        first: S,
    ) -> R<AsyncJsonLdStore<'_, B, S>> {
        self.backend
            .execute(&schema::create_schema(&options.indexes))
            .await
            .map_err(from_core)?;
        Ok(AsyncJsonLdStore {
            store: self,
            options,
            first,
        })
    }
}

impl<B: AsyncBackend, S: Loader> AsyncJsonLdStore<'_, B, S> {
    pub fn options(&self) -> &JsonLdOptions {
        &self.options
    }

    pub fn store(&self) -> &AsyncStore<B> {
        self.store
    }

    async fn run<J: Job>(&self, job: J) -> R<J::Output> {
        run_async(&self.store.backend, job).await.map_err(from_core)
    }

    async fn write(&self, puts: Vec<DocumentInput>, removes: Vec<String>) -> R<WriteOutcome> {
        let job = WriteJob::new(
            puts,
            removes,
            self.options.clone(),
            &self.first,
            self.store.caps().clone(),
        )?;
        self.run(job).await
    }

    /// Stores a document (one atomic request, one D1 batch) and returns its key.
    pub async fn put_document(&self, json: &str) -> R<String> {
        self.put_documents(vec![DocumentInput::new(json)])
            .await
            .map(|mut o| o.keys.remove(0))
    }

    pub async fn put_document_with_key(&self, key: &str, json: &str) -> R<String> {
        let mut input = DocumentInput::new(json);
        input.key = Some(key.to_owned());
        self.put_documents(vec![input])
            .await
            .map(|mut o| o.keys.remove(0))
    }

    pub async fn put_documents(&self, inputs: Vec<DocumentInput>) -> R<WriteOutcome> {
        self.write(inputs, Vec::new()).await
    }

    pub async fn get_document(&self, key: &str) -> R<Option<StoredDocument>> {
        Ok(self.run(get_job(key)).await?.into_iter().next())
    }

    pub async fn remove_document(&self, key: &str) -> R<bool> {
        Ok(self
            .write(Vec::new(), vec![key.to_owned()])
            .await?
            .removed
            .first()
            .copied()
            .unwrap_or(false))
    }

    pub async fn list_documents(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> R<Vec<StoredDocument>> {
        self.run(list_job(after, limit)).await
    }

    pub async fn find_documents(&self, filter: &DocumentFilter) -> R<Vec<StoredDocument>> {
        self.run(find_job(filter)).await
    }

    pub async fn document_graphs(&self, key: &str) -> R<Vec<GraphName>> {
        self.run(document_graphs_job(key)).await
    }

    pub async fn document_for_graph<'b>(
        &self,
        graph: impl Into<GraphNameRef<'b>>,
    ) -> R<Option<StoredDocument>> {
        Ok(self
            .run(document_for_graph_job(graph.into()))
            .await?
            .into_iter()
            .next())
    }

    pub async fn put_context(&self, iri: &str, json: &str) -> R<()> {
        self.store
            .backend
            .execute(&put_context_request(iri, json)?)
            .await
            .map_err(from_core)?;
        Ok(())
    }

    pub async fn remove_context(&self, iri: &str) -> R<()> {
        self.store
            .backend
            .execute(&remove_context_request(iri))
            .await
            .map_err(from_core)?;
        Ok(())
    }

    pub async fn contexts(&self) -> R<Vec<String>> {
        self.run(list_contexts_job()).await
    }

    pub async fn rebuild_graph(&self, key: &str) -> R<bool> {
        let job = RebuildJob::new(
            key,
            self.options.clone(),
            &self.first,
            self.store.caps().clone(),
        );
        Ok(self.run(job).await?.is_some())
    }

    pub async fn check_documents(&self) -> R<Vec<Drift>> {
        self.run(CheckJob::new(self.options.clone(), &self.first))
            .await
    }
}

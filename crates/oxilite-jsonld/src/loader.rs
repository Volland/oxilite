//! Context loading without I/O: registered contexts, an optional first loader (bundled
//! contexts), then contexts read from `jsonld_contexts` by the job itself.
//!
//! Loaders never await anything real, so the JSON-LD futures complete on their first poll
//! (see [`poll_ready`]). A context that is not available is recorded as a *miss*; the job
//! then reads the misses from the database (or fetches them) and converts again.
//!
// @lat: [[architecture#JSON-LD documents#Context loading]]

use crate::error::JsonLdError;
use iref::{Iri, IriBuf};
use json_ld::{LoadError, Loader, RemoteDocument};
use json_syntax::Parse;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

/// A loader that knows nothing (the default first loader).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoContexts;

impl Loader for NoContexts {
    async fn load(&self, url: &Iri) -> Result<RemoteDocument<IriBuf>, LoadError> {
        Err(LoadError::new(url.to_owned(), NotAvailable))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("context not available offline")]
struct NotAvailable;

/// The chain used during one conversion: `known` (registered, persisted or fetched contexts,
/// JSON text by IRI), then `first` (e.g. bundled credential contexts). Misses are recorded.
pub(crate) struct ChainLoader<'a, S> {
    pub known: &'a BTreeMap<String, String>,
    pub first: &'a S,
    pub misses: RefCell<BTreeSet<String>>,
}

impl<'a, S: Loader> ChainLoader<'a, S> {
    pub fn new(known: &'a BTreeMap<String, String>, first: &'a S) -> Self {
        Self {
            known,
            first,
            misses: RefCell::default(),
        }
    }
}

impl<S: Loader> Loader for ChainLoader<'_, S> {
    async fn load(&self, url: &Iri) -> Result<RemoteDocument<IriBuf>, LoadError> {
        if let Some(text) = self.known.get(url.as_str()) {
            return match json_syntax::Value::parse_str(text) {
                Ok((v, _)) => Ok(RemoteDocument::new(
                    Some(url.to_owned()),
                    "application/ld+json".parse().ok(),
                    v,
                )),
                Err(e) => Err(LoadError::new(
                    url.to_owned(),
                    InvalidContext(e.to_string()),
                )),
            };
        }
        match self.first.load(url).await {
            Ok(d) => Ok(d),
            Err(e) => {
                self.misses.borrow_mut().insert(url.as_str().to_owned());
                Err(e)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid context JSON: {0}")]
struct InvalidContext(String);

/// Polls a future that must not wait on anything (all our loaders are in memory).
pub(crate) fn poll_ready<F: Future>(f: F) -> Result<F::Output, JsonLdError> {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    // A few polls tolerate yield points inside the JSON-LD processor.
    for _ in 0..64 {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return Ok(v);
        }
    }
    Err(JsonLdError::Store(oxilite_core::Error::Other(
        "JSON-LD processing waited on I/O; use a context fetcher instead of an async loader".into(),
    )))
}

/// A fetcher downloading contexts over HTTP(S) (`network` feature, native only).
#[cfg(feature = "network")]
pub fn http_fetcher() -> crate::options::ContextFetcher {
    std::sync::Arc::new(|iri: &str| {
        let resp = ureq::get(iri)
            .set("Accept", "application/ld+json, application/json")
            .call()
            .map_err(|e| JsonLdError::ContextNotFound(format!("{iri}: {e}")))?;
        resp.into_string()
            .map_err(|e| JsonLdError::ContextNotFound(format!("{iri}: {e}")))
    })
}

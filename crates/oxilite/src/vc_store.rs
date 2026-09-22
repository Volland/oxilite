//! Verifiable Credentials on the blocking and async stores (`Store::credentials`).
//!
// @lat: [[architecture#JSON-LD documents#Verifiable Credentials]]

use crate::jsonld_store::{AsyncJsonLdStore, JsonLdStore};
use crate::store::Store;
use crate::AsyncStore;
use oxilite_core::{AsyncBackend, SyncBackend};
use oxilite_jsonld::{DocumentInput, JsonLdError, StoredDocument};
use oxilite_vc::{
    bundled_contexts, credential_input, presentation_inputs, BundledContexts, CredentialFilter,
    CredentialOptions, PresentationKeys,
};

type R<T> = Result<T, JsonLdError>;

/// Credentials stored in a [`Store`]: verbatim under their `id`, RDF in the graph of the same
/// IRI, issuer/subject/type/validity indexed.
///
/// ```
/// use oxilite::store::Store;
///
/// let store = Store::new()?;
/// let vcs = store.credentials()?;
/// let key = vcs.put_credential(r#"{
///   "@context": ["https://www.w3.org/ns/credentials/v2"],
///   "id": "urn:uuid:3978344f-8596-4c3a-a978-8fcaba3903c5",
///   "type": ["VerifiableCredential"],
///   "issuer": "did:example:issuer",
///   "validFrom": "2024-01-01T00:00:00Z",
///   "credentialSubject": {"id": "did:example:alice"}
/// }"#)?;
/// assert_eq!(key, "urn:uuid:3978344f-8596-4c3a-a978-8fcaba3903c5");
/// let ask = "ASK { GRAPH <urn:uuid:3978344f-8596-4c3a-a978-8fcaba3903c5> {
///     ?vc a <https://www.w3.org/2018/credentials#VerifiableCredential> } }";
/// assert!(matches!(store.query(ask)?, oxilite::sparql::QueryResults::Boolean(true)));
/// let found = vcs.find_credentials(&oxilite::vc::CredentialFilter {
///     issuer: Some("did:example:issuer".into()),
///     ..Default::default()
/// })?;
/// assert_eq!(found.len(), 1);
/// # Result::<_, Box<dyn std::error::Error>>::Ok(())
/// ```
pub struct CredentialStore<'a, B: SyncBackend> {
    docs: JsonLdStore<'a, B, BundledContexts>,
    options: CredentialOptions,
}

impl<B: SyncBackend + Send + Sync + 'static> Store<B> {
    /// Credentials with the default options (key = `id`, graph = key).
    pub fn credentials(&self) -> R<CredentialStore<'_, B>> {
        self.credentials_with(CredentialOptions::default())
    }

    /// Credentials with explicit options.
    pub fn credentials_with(&self, options: CredentialOptions) -> R<CredentialStore<'_, B>> {
        Ok(CredentialStore {
            docs: self.jsonld_with_loader(options.jsonld.clone(), bundled_contexts())?,
            options,
        })
    }
}

impl<B: SyncBackend + Send + Sync + 'static> CredentialStore<'_, B> {
    /// The underlying document handle (graphs, contexts, `check_documents`…).
    pub fn documents(&self) -> &JsonLdStore<'_, B, BundledContexts> {
        &self.docs
    }

    /// Checks and stores a credential (replacing one with the same key); returns its key.
    pub fn put_credential(&self, json: &str) -> R<String> {
        self.put(credential_input(json, None)?)
    }

    /// Stores a credential under an explicit key.
    pub fn put_credential_with_key(&self, key: &str, json: &str) -> R<String> {
        self.put(credential_input(json, Some(key.to_owned()))?)
    }

    fn put(&self, input: DocumentInput) -> R<String> {
        Ok(self.docs.put_documents(vec![input])?.keys.remove(0))
    }

    /// Stores a presentation and, by default, each embedded credential on its own — all in
    /// one atomic request.
    pub fn put_presentation(&self, json: &str) -> R<PresentationKeys> {
        let inputs = presentation_inputs(json, None, &self.options)?;
        let mut keys = self.docs.put_documents(inputs)?.keys;
        let key = keys.remove(0);
        Ok(PresentationKeys {
            key,
            credentials: keys,
        })
    }

    /// The stored credential (or presentation), byte for byte.
    pub fn get_credential(&self, key: &str) -> R<Option<StoredDocument>> {
        self.docs.get_document(key)
    }

    /// Removes a credential and its graphs (claims and proofs).
    pub fn remove_credential(&self, key: &str) -> R<bool> {
        self.docs.remove_document(key)
    }

    /// Credentials by issuer, subject, type, validity instant and profile, from the indexed
    /// columns (no SPARQL).
    pub fn find_credentials(&self, filter: &CredentialFilter) -> R<Vec<StoredDocument>> {
        self.docs.find_documents(filter)
    }
}

/// [`CredentialStore`] over an [`AsyncStore`] (Cloudflare D1).
pub struct AsyncCredentialStore<'a, B: AsyncBackend> {
    docs: AsyncJsonLdStore<'a, B, BundledContexts>,
    options: CredentialOptions,
}

impl<B: AsyncBackend> AsyncStore<B> {
    pub async fn credentials(&self) -> R<AsyncCredentialStore<'_, B>> {
        self.credentials_with(CredentialOptions::default()).await
    }

    pub async fn credentials_with(
        &self,
        options: CredentialOptions,
    ) -> R<AsyncCredentialStore<'_, B>> {
        Ok(AsyncCredentialStore {
            docs: self
                .jsonld_with_loader(options.jsonld.clone(), bundled_contexts())
                .await?,
            options,
        })
    }
}

impl<B: AsyncBackend> AsyncCredentialStore<'_, B> {
    pub fn documents(&self) -> &AsyncJsonLdStore<'_, B, BundledContexts> {
        &self.docs
    }

    pub async fn put_credential(&self, json: &str) -> R<String> {
        let input = credential_input(json, None)?;
        Ok(self.docs.put_documents(vec![input]).await?.keys.remove(0))
    }

    pub async fn put_credential_with_key(&self, key: &str, json: &str) -> R<String> {
        let input = credential_input(json, Some(key.to_owned()))?;
        Ok(self.docs.put_documents(vec![input]).await?.keys.remove(0))
    }

    pub async fn put_presentation(&self, json: &str) -> R<PresentationKeys> {
        let inputs = presentation_inputs(json, None, &self.options)?;
        let mut keys = self.docs.put_documents(inputs).await?.keys;
        let key = keys.remove(0);
        Ok(PresentationKeys {
            key,
            credentials: keys,
        })
    }

    pub async fn get_credential(&self, key: &str) -> R<Option<StoredDocument>> {
        self.docs.get_document(key).await
    }

    pub async fn remove_credential(&self, key: &str) -> R<bool> {
        self.docs.remove_document(key).await
    }

    pub async fn find_credentials(&self, filter: &CredentialFilter) -> R<Vec<StoredDocument>> {
        self.docs.find_documents(filter).await
    }
}

//! Verifiable Credentials in oxilite, on top of [`oxilite_jsonld`].
//!
//! A credential is stored verbatim under its `id`, its RDF goes into the named graph of the
//! same IRI, and its issuer, subject, types and validity window are indexed for lookups
//! without SPARQL. Structure is checked with [`ssi_vc`] (VCDM v1.1 and v2.0); the W3C
//! credential, security, Data Integrity and DID contexts come bundled with
//! [`ssi_json_ld`], so credentials convert offline, on D1 too.
//!
//! Proofs are **not** verified: storing a credential is not a claim that it is valid.
//! Use the `ssi` crates on the JSON returned by `get_credential` for that.
//!
//! This crate is sans-IO; the `oxilite` crate (feature `vc`) provides `Store::credentials()`.
//!
// @lat: [[architecture#JSON-LD documents#Verifiable Credentials]]

use json_syntax::{Parse, Print};
use oxilite_jsonld::{
    DocumentFilter, DocumentInput, DocumentMeta, JsonLdError, JsonLdOptions, MissingKey,
};
use std::fmt;

pub use oxilite_jsonld::{GraphStrategy, KeyStrategy, MetadataIndexes, StoredDocument};

/// Context loader of the bundled W3C contexts (credentials v1/v2, security, Data Integrity,
/// cryptosuites, DID v1…).
pub type BundledContexts = ssi_json_ld::ContextLoader;

/// The bundled contexts, used before persisted ones.
pub fn bundled_contexts() -> BundledContexts {
    ssi_json_ld::ContextLoader::empty().with_static_loader()
}

/// Credential lookups: [`DocumentFilter`] over credential metadata.
pub type CredentialFilter = DocumentFilter;

/// The first `@context` of VCDM 1.1.
pub const CREDENTIALS_V1: &str = "https://www.w3.org/2018/credentials/v1";
/// The first `@context` of VCDM 2.0.
pub const CREDENTIALS_V2: &str = "https://www.w3.org/ns/credentials/v2";

const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// Options of a credential handle.
#[derive(Debug, Clone)]
pub struct CredentialOptions {
    /// Keys, graphs, contexts and indexes (by default: key = `id`, graph = key, credentials
    /// without `id` keyed by content hash).
    pub jsonld: JsonLdOptions,
    /// Also store each credential embedded in a presentation as a document of its own.
    pub embed_presentation_credentials: bool,
}

impl Default for CredentialOptions {
    fn default() -> Self {
        Self {
            jsonld: JsonLdOptions {
                on_missing_key: MissingKey::ContentHash,
                ..Default::default()
            },
            embed_presentation_credentials: true,
        }
    }
}

/// Data model version of a credential or presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Vc1,
    Vc2,
    Vp1,
    Vp2,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vc1 => "vc1",
            Self::Vc2 => "vc2",
            Self::Vp1 => "vp1",
            Self::Vp2 => "vp2",
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Keys written by `put_presentation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationKeys {
    /// The presentation's key.
    pub key: String,
    /// Keys of the embedded credentials stored on their own.
    pub credentials: Vec<String>,
}

fn invalid(msg: impl Into<String>) -> JsonLdError {
    JsonLdError::Invalid(msg.into())
}

/// The data model version, from the first `@context`.
fn version(doc: &serde_json::Value) -> Result<u8, JsonLdError> {
    let first = match &doc["@context"] {
        serde_json::Value::Array(a) => a.first(),
        v @ serde_json::Value::String(_) => Some(v),
        _ => None,
    };
    match first.and_then(serde_json::Value::as_str) {
        Some(CREDENTIALS_V1) => Ok(1),
        Some(CREDENTIALS_V2) => Ok(2),
        _ => Err(invalid(format!(
            "the first @context must be {CREDENTIALS_V2} (VCDM 2.0) or {CREDENTIALS_V1} (VCDM 1.1)"
        ))),
    }
}

fn parse(json: &str) -> Result<serde_json::Value, JsonLdError> {
    serde_json::from_str(json).map_err(|e| JsonLdError::Json(e.to_string()))
}

fn id_of(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(o) => o.get("id").and_then(|i| i.as_str()).map(str::to_owned),
        _ => None,
    }
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::String(s) => vec![s.clone()],
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn time(v: &serde_json::Value) -> Option<f64> {
    oxilite_core::encoding::timestamp(v.as_str()?, XSD_DATE_TIME)
}

/// Checks a credential's structure and extracts its metadata.
pub fn credential_input(json: &str, key: Option<String>) -> Result<DocumentInput, JsonLdError> {
    let doc = parse(json)?;
    let profile = match version(&doc)? {
        1 => {
            serde_json::from_value::<ssi_vc::v1::JsonCredential>(doc.clone())
                .map_err(|e| invalid(format!("not a VCDM 1.1 credential: {e}")))?;
            Profile::Vc1
        }
        _ => {
            serde_json::from_value::<ssi_vc::v2::JsonCredential>(doc.clone())
                .map_err(|e| invalid(format!("not a VCDM 2.0 credential: {e}")))?;
            Profile::Vc2
        }
    };
    let subject = match &doc["credentialSubject"] {
        serde_json::Value::Array(a) => a.iter().find_map(id_of),
        v => id_of(v),
    };
    let meta = DocumentMeta {
        profile: profile.as_str().into(),
        issuer: id_of(&doc["issuer"]),
        subject,
        types: strings(&doc["type"]),
        valid_from: time(&doc["validFrom"]).or_else(|| time(&doc["issuanceDate"])),
        valid_until: time(&doc["validUntil"]).or_else(|| time(&doc["expirationDate"])),
        refs: Vec::new(),
    };
    Ok(DocumentInput {
        json: json.to_owned(),
        key,
        meta,
    })
}

/// Checks a presentation and prepares it, plus (when enabled) each embedded credential as a
/// document of its own. The presentation comes first.
pub fn presentation_inputs(
    json: &str,
    key: Option<String>,
    options: &CredentialOptions,
) -> Result<Vec<DocumentInput>, JsonLdError> {
    let doc = parse(json)?;
    let v = version(&doc)?;
    let profile = if v == 1 {
        serde_json::from_value::<ssi_vc::v1::JsonPresentation<json_syntax::Value>>(doc.clone())
            .map_err(|e| invalid(format!("not a VCDM 1.1 presentation: {e}")))?;
        Profile::Vp1
    } else {
        serde_json::from_value::<ssi_vc::v2::syntax::JsonPresentation<json_syntax::Value>>(
            doc.clone(),
        )
        .map_err(|e| invalid(format!("not a VCDM 2.0 presentation: {e}")))?;
        Profile::Vp2
    };
    let mut embedded = Vec::new();
    if options.embed_presentation_credentials {
        // Re-read with json-syntax, which keeps member order, to print embedded credentials.
        let (tree, _) =
            json_syntax::Value::parse_str(json).map_err(|e| JsonLdError::Json(e.to_string()))?;
        let vcs = tree
            .as_object()
            .and_then(|o| o.get_unique("verifiableCredential").ok().flatten());
        let items: Vec<&json_syntax::Value> = match vcs {
            Some(json_syntax::Value::Array(a)) => a.iter().collect(),
            Some(v) => vec![v],
            None => Vec::new(),
        };
        for item in items {
            let Some(obj) = item.as_object() else {
                continue;
            };
            // Enveloped credentials (VC-JOSE/COSE) carry no JSON-LD body to store.
            let types = obj
                .get_unique("type")
                .ok()
                .flatten()
                .map(|t| t.compact_print().to_string())
                .unwrap_or_default();
            if types.contains("EnvelopedVerifiableCredential") {
                continue;
            }
            let text = item.compact_print().to_string();
            let mut input = credential_input(&text, None)
                .map_err(|e| invalid(format!("embedded credential: {e}")))?;
            let (tree, _) = json_syntax::Value::parse_str(&text)
                .map_err(|e| JsonLdError::Json(e.to_string()))?;
            let k = options.jsonld.key_for(&text, &tree, None)?;
            input.key = Some(k);
            embedded.push(input);
        }
    }
    let holder = id_of(&doc["holder"]);
    let mut vp = DocumentInput {
        json: json.to_owned(),
        key,
        meta: DocumentMeta {
            profile: profile.as_str().into(),
            issuer: None,
            subject: holder,
            types: strings(&doc["type"]),
            valid_from: None,
            valid_until: None,
            refs: embedded.iter().filter_map(|e| e.key.clone()).collect(),
        },
    };
    if vp.key.is_none() {
        let (tree, _) =
            json_syntax::Value::parse_str(json).map_err(|e| JsonLdError::Json(e.to_string()))?;
        vp.key = Some(options.jsonld.key_for(json, &tree, None)?);
    }
    let mut out = vec![vp];
    out.extend(embedded);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_versions_and_metadata() {
        let v1 = r#"{"@context": ["https://www.w3.org/2018/credentials/v1"], "id": "urn:uuid:1", "type": ["VerifiableCredential"], "issuer": {"id": "did:example:i"}, "issuanceDate": "2020-01-01T00:00:00Z", "expirationDate": "2030-01-01T00:00:00Z", "credentialSubject": {"id": "did:example:s"}}"#;
        let i = credential_input(v1, None).unwrap();
        assert_eq!(i.meta.profile, "vc1");
        assert_eq!(i.meta.issuer.as_deref(), Some("did:example:i"));
        assert_eq!(i.meta.subject.as_deref(), Some("did:example:s"));
        assert_eq!(i.meta.valid_from, Some(1_577_836_800.0));
        assert_eq!(i.meta.valid_until, Some(1_893_456_000.0));
        let bad = r#"{"@context": ["https://example.org/ctx"], "type": ["VerifiableCredential"]}"#;
        assert!(matches!(
            credential_input(bad, None),
            Err(JsonLdError::Invalid(_))
        ));
    }
}

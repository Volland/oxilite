//! JSON-LD → RDF with the `json-ld` crate, then `rdf-types` → `oxrdf`.
//!
// @lat: [[architecture#JSON-LD documents#Conversion]]

use crate::error::JsonLdError;
use crate::loader::{poll_ready, ChainLoader};
use crate::options::{JsonLdOptions, ProcessingMode, RdfDirection};
use iref::IriBuf;
use json_ld::{JsonLdProcessor, Loader, RemoteDocument};
use oxrdf::{BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, Term};
use rdf_types::{Id, LiteralType};
use std::collections::{BTreeMap, BTreeSet};

/// The RDF of one document.
#[derive(Debug, Default)]
pub struct Converted {
    /// Quads, with the document's default graph already rewritten to the target graph.
    pub quads: Vec<Quad>,
    /// Named graphs the document itself defines (`@graph` containers, graph objects).
    pub nested_graphs: Vec<GraphName>,
}

/// Outcome of one conversion attempt.
pub(crate) enum Attempt {
    Done(Converted),
    /// These context IRIs were not available.
    Missing(BTreeSet<String>),
}

/// Blank-node label prefix of a document: `d<16 hex of xxh3-128(key)>_`.
pub fn blank_prefix(key: &str) -> String {
    let h = xxhash_rust::xxh3::xxh3_128(key.as_bytes());
    format!("d{:016x}_", (h >> 64) as u64)
}

pub(crate) fn convert<S: Loader>(
    doc: &json_syntax::Value,
    key: &str,
    target: &GraphName,
    options: &JsonLdOptions,
    known: &BTreeMap<String, String>,
    first: &S,
) -> Result<Attempt, JsonLdError> {
    let loader = ChainLoader::new(known, first);
    let base = match &options.base_iri {
        Some(b) => Some(
            IriBuf::new(b.clone())
                .map_err(|e| JsonLdError::Invalid(format!("invalid base IRI {b}: {}", e.0)))?,
        ),
        None => None,
    };
    let remote = RemoteDocument::new(
        base.clone(),
        "application/ld+json".parse().ok(),
        doc.clone(),
    );
    let generator = rdf_types::generator::Blank::new_with_prefix(blank_prefix(key));
    let jl_options = json_ld::Options {
        base,
        processing_mode: match options.processing_mode {
            ProcessingMode::JsonLd10 => json_ld::ProcessingMode::JsonLd1_0,
            ProcessingMode::JsonLd11 => json_ld::ProcessingMode::JsonLd1_1,
        },
        rdf_direction: options.rdf_direction.map(|d| match d {
            RdfDirection::I18nDatatype => json_ld::rdf::RdfDirection::I18nDatatype,
            RdfDirection::CompoundLiteral => json_ld::rdf::RdfDirection::CompoundLiteral,
        }),
        ..Default::default()
    };
    let result = poll_ready(remote.to_rdf_using(generator, &loader, jl_options))?;
    let misses = loader.misses.take();
    let mut to_rdf = match result {
        Ok(r) => r,
        Err(e) => {
            if !misses.is_empty() {
                return Ok(Attempt::Missing(misses));
            }
            return Err(JsonLdError::JsonLd {
                code: e.code().to_string(),
                message: e.to_string(),
            });
        }
    };
    let mut out = Converted::default();
    for q in to_rdf.cloned_quads() {
        let rdf_types::Quad(s, p, o, g) = q;
        let (Some(s), Some(p)) = (subject(s), predicate(p)) else {
            continue; // generalized RDF (blank predicates) is not stored
        };
        let Some(o) = object(o) else { continue };
        let graph = match g {
            None => target.clone(),
            Some(g) => {
                let g = match subject(g) {
                    Some(NamedOrBlankNode::NamedNode(n)) => GraphName::NamedNode(n),
                    Some(NamedOrBlankNode::BlankNode(b)) => GraphName::BlankNode(b),
                    None => continue,
                };
                if !out.nested_graphs.contains(&g) {
                    out.nested_graphs.push(g.clone());
                }
                g
            }
        };
        out.quads.push(Quad::new(s, p, o, graph));
    }
    Ok(Attempt::Done(out))
}

fn subject(id: Id) -> Option<NamedOrBlankNode> {
    Some(match id {
        Id::Iri(i) => NamedNode::new(i.into_string()).ok()?.into(),
        Id::Blank(b) => blank(b.as_str())?.into(),
    })
}

fn predicate(id: Id) -> Option<NamedNode> {
    match id {
        Id::Iri(i) => NamedNode::new(i.into_string()).ok(),
        Id::Blank(_) => None,
    }
}

fn blank(label: &str) -> Option<BlankNode> {
    BlankNode::new(label.strip_prefix("_:").unwrap_or(label)).ok()
}

fn object(o: rdf_types::Term) -> Option<Term> {
    Some(match o {
        rdf_types::Term::Id(id) => subject(id)?.into(),
        rdf_types::Term::Literal(l) => match l.type_ {
            LiteralType::Any(dt) => {
                let dt = i18n_datatype(dt.into_string());
                Literal::new_typed_literal(l.value, NamedNode::new(dt).ok()?).into()
            }
            LiteralType::LangString(lang) => {
                Literal::new_language_tagged_literal(l.value, lang.as_str())
                    .ok()?
                    .into()
            }
        },
    })
}

const I18N: &str = "https://www.w3.org/ns/i18n#";

/// `rdfDirection: i18n-datatype` IRIs as JSON-LD 1.1 defines them: lower-case language,
/// then `_` and the direction (`json-ld` 0.21 omits the `_` without a language and keeps
/// the language's case).
fn i18n_datatype(dt: String) -> String {
    let Some(frag) = dt.strip_prefix(I18N) else {
        return dt;
    };
    match frag.rsplit_once('_') {
        Some((lang, dir)) => format!("{I18N}{}_{dir}", lang.to_ascii_lowercase()),
        None => format!("{I18N}_{frag}"),
    }
}

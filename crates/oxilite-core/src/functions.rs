//! Host functions: user code callable from SPARQL, Cypher and Datalog.
//!
//! A host function maps RDF terms to an optional RDF term, the signature of spareval's custom
//! functions: `None` is an evaluation error, which SPARQL turns into an unbound value. Functions
//! are registered on a store under an IRI (plus an optional Cypher name) and are never
//! persisted: they are code, not data. Each frontend decides where the call runs — SPARQL and
//! Cypher through the fallback evaluator, Datalog in a Rust pass over a rule's rows — and every
//! part of a query that does not call one still compiles to SQL.
//!
// @lat: [[architecture#Host functions]]

use crate::error::{Error, Result};
use oxrdf::{NamedNode, Term};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The callable of a host function.
pub type HostFn = Arc<dyn Fn(&[Term]) -> Option<Term> + Send + Sync>;

/// A function implemented by the application.
#[derive(Clone)]
pub struct HostFunction {
    iri: String,
    cypher_name: Option<String>,
    min_arity: usize,
    max_arity: Option<usize>,
    description: String,
    f: HostFn,
}

impl std::fmt::Debug for HostFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostFunction")
            .field("iri", &self.iri)
            .field("cypher_name", &self.cypher_name)
            .field("min_arity", &self.min_arity)
            .field("max_arity", &self.max_arity)
            .finish_non_exhaustive()
    }
}

impl HostFunction {
    /// A function named by `iri` (checked when it is registered).
    pub fn new(
        iri: impl Into<String>,
        f: impl Fn(&[Term]) -> Option<Term> + Send + Sync + 'static,
    ) -> Self {
        Self {
            iri: iri.into(),
            cypher_name: None,
            min_arity: 0,
            max_arity: None,
            description: String::new(),
            f: Arc::new(f),
        }
    }

    /// The name Cypher calls it by (default: the local name of the IRI).
    pub fn cypher_name(mut self, name: impl Into<String>) -> Self {
        self.cypher_name = Some(name.into());
        self
    }

    /// How many arguments it takes; a call with another number is an evaluation error.
    pub fn arity(mut self, min: usize, max: impl Into<Option<usize>>) -> Self {
        self.min_arity = min;
        self.max_arity = max.into();
        self
    }

    /// A one-line description, shown by tools (completion, `.functions`).
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = text.into();
        self
    }

    pub fn iri(&self) -> &str {
        &self.iri
    }

    /// The name Cypher calls it by: the explicit one, else the IRI's local name.
    pub fn cypher(&self) -> &str {
        self.cypher_name
            .as_deref()
            .unwrap_or_else(|| local_name(&self.iri))
    }

    pub fn min_arity(&self) -> usize {
        self.min_arity
    }

    pub fn max_arity(&self) -> Option<usize> {
        self.max_arity
    }

    pub fn description_text(&self) -> &str {
        &self.description
    }

    /// Calls the function; a wrong number of arguments is an evaluation error (`None`).
    pub fn call(&self, args: &[Term]) -> Option<Term> {
        if args.len() < self.min_arity || self.max_arity.is_some_and(|m| args.len() > m) {
            return None;
        }
        (self.f)(args)
    }

    /// The callable, arity check included, as spareval takes it.
    pub fn callable(&self) -> impl Fn(&[Term]) -> Option<Term> + Send + Sync + 'static {
        let me = self.clone();
        move |args| me.call(args)
    }
}

/// The part of an IRI after its last `#` or `/`.
pub fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// IRIs the engine interprets itself, which a host function may not take over.
fn reserved(iri: &str) -> Option<&'static str> {
    if iri == crate::text::TEXT_MATCH || iri.starts_with(crate::registry::NS) {
        Some("the oxilite namespace is reserved")
    } else if iri.starts_with("http://www.w3.org/2001/XMLSchema#") {
        Some("XSD casts are built in")
    } else {
        None
    }
}

/// The host functions of a store, by IRI.
#[derive(Clone, Default, Debug)]
pub struct FunctionRegistry {
    by_iri: BTreeMap<String, HostFunction>,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a function, replacing one with the same IRI. Fails on an invalid or reserved IRI,
    /// and on a Cypher name another function already uses.
    pub fn register(&mut self, function: HostFunction) -> Result<()> {
        NamedNode::new(function.iri.as_str()).map_err(|e| {
            Error::Other(format!(
                "host function IRI <{}> is invalid: {e}",
                function.iri
            ))
        })?;
        if let Some(why) = reserved(&function.iri) {
            return Err(Error::Other(format!(
                "cannot register <{}> as a host function: {why}",
                function.iri
            )));
        }
        let name = function.cypher().to_lowercase();
        if name.is_empty() {
            return Err(Error::Other(format!(
                "host function <{}> needs a Cypher name (its IRI has no local name)",
                function.iri
            )));
        }
        if let Some(other) = self
            .by_iri
            .values()
            .find(|f| f.iri != function.iri && f.cypher().to_lowercase() == name)
        {
            return Err(Error::Other(format!(
                "Cypher name {} is already used by <{}>",
                function.cypher(),
                other.iri
            )));
        }
        self.by_iri.insert(function.iri.clone(), function);
        Ok(())
    }

    /// Removes a function; `false` when none has this IRI.
    pub fn unregister(&mut self, iri: &str) -> bool {
        self.by_iri.remove(iri).is_some()
    }

    pub fn get(&self, iri: &str) -> Option<&HostFunction> {
        self.by_iri.get(iri)
    }

    /// The function Cypher calls `name` (case-insensitive).
    pub fn by_cypher_name(&self, name: &str) -> Option<&HostFunction> {
        self.by_iri
            .values()
            .find(|f| f.cypher().eq_ignore_ascii_case(name))
    }

    pub fn iter(&self) -> impl Iterator<Item = &HostFunction> {
        self.by_iri.values()
    }

    pub fn is_empty(&self) -> bool {
        self.by_iri.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_iri.len()
    }

    /// Adds every function to a spareval evaluator.
    pub fn install(&self, mut evaluator: spareval::QueryEvaluator) -> spareval::QueryEvaluator {
        for f in self.by_iri.values() {
            evaluator =
                evaluator.with_custom_function(NamedNode::new_unchecked(f.iri()), f.callable());
        }
        evaluator
    }
}

/// A shared, possibly absent registry: what stores hand to `QueryOptions` and the frontends'
/// options. Two handles are equal when they share one registry.
#[derive(Clone, Default, Debug)]
pub struct Functions(pub Option<Arc<FunctionRegistry>>);

impl Functions {
    pub fn new(registry: FunctionRegistry) -> Self {
        Self(Some(Arc::new(registry)))
    }

    pub fn registry(&self) -> Option<&FunctionRegistry> {
        self.0.as_deref()
    }

    pub fn get(&self, iri: &str) -> Option<&HostFunction> {
        self.registry()?.get(iri)
    }

    pub fn by_cypher_name(&self, name: &str) -> Option<&HostFunction> {
        self.registry()?.by_cypher_name(name)
    }

    pub fn is_empty(&self) -> bool {
        self.registry().is_none_or(FunctionRegistry::is_empty)
    }

    /// Adds every function to a spareval evaluator.
    pub fn install(&self, evaluator: spareval::QueryEvaluator) -> spareval::QueryEvaluator {
        match self.registry() {
            Some(r) => r.install(evaluator),
            None => evaluator,
        }
    }
}

impl PartialEq for Functions {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for Functions {}

#[cfg(test)]
mod tests {
    use super::*;
    use oxrdf::Literal;

    fn upper() -> HostFunction {
        HostFunction::new("http://example.com/fn#upper", |args| {
            match args.first()? {
                Term::Literal(l) => {
                    Some(Literal::new_simple_literal(l.value().to_uppercase()).into())
                }
                _ => None,
            }
        })
        .arity(1, 1)
    }

    #[test]
    fn registry_names_and_arity() {
        let mut r = FunctionRegistry::new();
        r.register(upper()).unwrap();
        let f = r.by_cypher_name("UPPER").unwrap();
        assert_eq!(f.iri(), "http://example.com/fn#upper");
        assert_eq!(
            f.call(&[Literal::new_simple_literal("a").into()]),
            Some(Literal::new_simple_literal("A").into())
        );
        assert_eq!(f.call(&[]), None);
        assert!(r
            .register(HostFunction::new("http://other/upper", |_| None))
            .is_err());
        assert!(r
            .register(HostFunction::new(crate::text::TEXT_MATCH, |_| None))
            .is_err());
        assert!(r
            .register(HostFunction::new("not an iri", |_| None))
            .is_err());
        assert!(r.unregister("http://example.com/fn#upper"));
        assert!(r.is_empty());
    }
}

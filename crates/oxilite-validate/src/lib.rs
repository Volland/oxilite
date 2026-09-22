//! SHACL and ShEx validation of oxilite data with [rudof](https://rudof-project.github.io/).
//!
//! rudof's engines run unchanged: [`StoreGraph`] implements rudof's `Rdf`, `NeighsRDF` and
//! `QueryRDF` traits over a blocking [`Store`], so neighbourhood lookups become SQL pattern
//! scans and rudof's SPARQL runs through the oxilite compiler. For stores that cannot block
//! (Cloudflare D1), [`prefetch`] loads the relevant subgraph into rudof's in-memory graph with
//! a bounded number of requests, then validates it natively. rudof's validators do not build
//! for wasm32, so D1 validation runs in native code (a CLI, a server, CI) over the D1 HTTP API.
//!
// @lat: [[architecture#Validation]]

mod graph;
pub mod prefetch;

pub use graph::StoreGraph;
pub use shacl::validator::report::ValidationReport;
pub use shacl::validator::ShaclValidationMode;
pub use shex_ast::shapemap::result_shape_map::ResultShapeMap;

use oxilite::store::Store;
use oxilite_core::SyncBackend;
use rudof_iri::IriS;
use rudof_rdf::rdf_core::RDFFormat;
use rudof_rdf::rdf_impl::{OxigraphInMemory, ReaderMode};
use shacl::ir::IRSchema;
use shacl::rdf::ShaclParser;
use shacl::validator::engine::{Engine, NativeEngine, SparqlEngine};
use shacl::validator::processor::ShaclProcessor;
use shacl::validator::ShaclConfig;
use shex_ast::compact::ShapeMapParser;
use shex_ast::ir::map_state::MapState;
use shex_ast::ir::schema_ir::SchemaIR;
use shex_ast::ir::semantic_actions_registry::SemanticActionsRegistry;
use shex_ast::{ResolveMethod, ShExParser};
use shex_validation::{Validator, ValidatorConfig};
use std::fmt::Debug;

/// Validation errors (shapes that do not parse, rudof failures, store errors, limits).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid shapes: {0}")]
    Shapes(String),
    #[error("validation failed: {0}")]
    Validation(String),
    #[error(transparent)]
    Store(#[from] oxilite_core::Error),
    #[error(
        "the subgraph needed for validation has more than {limit} triples ({found} found so far); raise the limit or validate natively"
    )]
    TooLarge { limit: usize, found: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Parses a SHACL shapes graph (Turtle, N-Triples, RDF/XML…) and compiles it.
pub fn shacl_schema(shapes: &str, format: &RDFFormat, base: Option<&str>) -> Result<IRSchema> {
    let graph = OxigraphInMemory::from_str(shapes, format, base, &ReaderMode::Strict)
        .map_err(|e| Error::Shapes(e.to_string()))?;
    let ast = ShaclParser::new(graph)
        .parse()
        .map_err(|e| Error::Shapes(e.to_string()))?;
    IRSchema::try_from(ast).map_err(|e| Error::Shapes(e.to_string()))
}

/// A SHACL processor over any rudof graph (a store, or a prefetched in-memory graph).
struct Processor<S> {
    data: S,
}

impl<
        S: rudof_rdf::rdf_core::NeighsRDF
            + rudof_rdf::rdf_core::query::QueryRDF
            + Debug
            + Send
            + Sync
            + 'static,
    > ShaclProcessor<S> for Processor<S>
{
    fn store(&self) -> &S {
        &self.data
    }

    fn runner(mode: &ShaclValidationMode, config: &ShaclConfig) -> Box<dyn Engine<S>> {
        match mode {
            ShaclValidationMode::Native => {
                Box::new(NativeEngine::new(config.recursion_semantics()))
            }
            ShaclValidationMode::Sparql => {
                Box::new(SparqlEngine::new(config.recursion_semantics()))
            }
        }
    }
}

/// Validates any rudof graph against compiled SHACL shapes.
pub fn validate_shacl_graph<S>(
    data: S,
    schema: &IRSchema,
    mode: &ShaclValidationMode,
) -> Result<ValidationReport>
where
    S: rudof_rdf::rdf_core::NeighsRDF
        + rudof_rdf::rdf_core::query::QueryRDF
        + Debug
        + Send
        + Sync
        + 'static,
{
    Processor { data }
        .validate(schema, mode, &ShaclConfig::default())
        .map_err(|e| Error::Validation(e.to_string()))
}

/// Validates the default graph of a store (or, with `union`, all graphs merged) against a
/// SHACL shapes graph given as Turtle.
pub fn validate_shacl<B: SyncBackend + Send + Sync + 'static>(
    store: &Store<B>,
    shapes_turtle: &str,
    mode: &ShaclValidationMode,
) -> Result<ValidationReport> {
    let schema = shacl_schema(shapes_turtle, &RDFFormat::Turtle, None)?;
    validate_shacl_graph(StoreGraph::new(store.clone()), &schema, mode)
}

/// Parses and compiles a ShEx schema (ShExC).
pub fn shex_schema(schema: &str, base: &str) -> Result<SchemaIR> {
    let base = IriS::new(base).map_err(|e| Error::Shapes(e.to_string()))?;
    let ast = ShExParser::parse(schema, Some(base.clone()), &base)
        .map_err(|e| Error::Shapes(e.to_string()))?;
    let config = ValidatorConfig::default();
    let mut map_state = MapState::default();
    let registry = SemanticActionsRegistry::default();
    registry.set_map_state(&mut map_state);
    let mut compiler = shex_ast::ir::ast2ir::AST2IR::new(&ResolveMethod::default(), map_state);
    let mut compiled = SchemaIR::new(registry);
    compiler
        .compile(
            &ast,
            &base,
            &Some(base.clone()),
            &mut compiled,
            config.external_resolvers(),
        )
        .map_err(|e| Error::Shapes(e.to_string()))?;
    Ok(compiled)
}

/// Validates the nodes of a ShEx shape map (compact syntax, e.g. `<http://ex/a>@<http://ex/S>`)
/// on any rudof graph.
pub fn validate_shex_graph<S>(
    data: &S,
    schema: &SchemaIR,
    shape_map: &str,
) -> Result<ResultShapeMap>
where
    S: rudof_rdf::rdf_core::NeighsRDF + rudof_rdf::rdf_core::query::QueryRDF,
{
    let nodes_pm = data.prefixmap();
    let shapes_pm = Some(schema.prefixmap());
    let map = ShapeMapParser::parse(shape_map, &nodes_pm, &None, &shapes_pm, &None)
        .map_err(|e| Error::Shapes(e.to_string()))?;
    let validator = Validator::new(schema, &ValidatorConfig::default())
        .map_err(|e| Error::Validation(e.to_string()))?;
    validator
        .validate_shapemap(&map, data, schema, &nodes_pm)
        .map_err(|e| Error::Validation(e.to_string()))
}

/// Validates the default graph of a store against a ShEx schema and a shape map.
pub fn validate_shex<B: SyncBackend + Send + Sync + 'static>(
    store: &Store<B>,
    shexc: &str,
    base: &str,
    shape_map: &str,
) -> Result<ResultShapeMap> {
    let schema = shex_schema(shexc, base)?;
    validate_shex_graph(&StoreGraph::new(store.clone()), &schema, shape_map)
}

//! RDFS / OWL reasoning: the schema closure, query rewriting and OWL 2 RL materialization.
//!
//! Query-time reasoning rewrites each triple pattern into a derived table of *entailed*
//! triples, computed from the asserted quads and `tbox_closure` (a small table recomputed by
//! `optimize()` and by writes that touch schema predicates). Queries therefore stay single SQL
//! statements. Materialization is explicit: OWL 2 RL rules run as `INSERT … SELECT`
//! statements into `quads_inf` until nothing changes.
//!
// @lat: [[architecture#Reasoning]]

use crate::encoding::{named_node_id, DEFAULT_GRAPH_ID, PAYLOAD_BITS};
use crate::sql::{union_all, Statement};
use oxrdf::vocab::{rdf, rdfs};
use oxrdf::{QuadRef, Term, TermRef};
use spargebra::term::{NamedNodePattern, TermPattern};
use spargebra::{GraphUpdateOperation, Update};
use std::collections::BTreeSet;

/// Entailment regime of a query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Reasoning {
    /// Asserted triples only (Oxigraph's behaviour).
    #[default]
    None,
    /// RDFS: subclass, subproperty, domain and range (plus equivalent classes and properties).
    Rdfs,
    /// RDFS plus the OWL QL property axioms: inverse and symmetric properties, and transitive
    /// properties.
    OwlQl,
}

const OWL: &str = "http://www.w3.org/2002/07/owl#";

fn owl(local: &str) -> i64 {
    named_node_id(&format!("{OWL}{local}"))
}

/// Ids of the vocabulary used by the closure and the rules.
#[derive(Debug, Clone, Copy)]
struct Vocab {
    ty: i64,
    sco: i64,
    spo: i64,
    dom: i64,
    rng: i64,
    eqc: i64,
    eqp: i64,
    inv: i64,
    sym: i64,
    trans: i64,
    same: i64,
    func: i64,
    ifunc: i64,
    has_value: i64,
    on_property: i64,
    some_values: i64,
    all_values: i64,
    intersection: i64,
    union: i64,
    chain: i64,
    first: i64,
    rest: i64,
    nil: i64,
    thing: i64,
}

fn vocab() -> Vocab {
    Vocab {
        ty: named_node_id(rdf::TYPE.as_str()),
        sco: named_node_id(rdfs::SUB_CLASS_OF.as_str()),
        spo: named_node_id(rdfs::SUB_PROPERTY_OF.as_str()),
        dom: named_node_id(rdfs::DOMAIN.as_str()),
        rng: named_node_id(rdfs::RANGE.as_str()),
        eqc: owl("equivalentClass"),
        eqp: owl("equivalentProperty"),
        inv: owl("inverseOf"),
        sym: owl("SymmetricProperty"),
        trans: owl("TransitiveProperty"),
        same: owl("sameAs"),
        func: owl("FunctionalProperty"),
        ifunc: owl("InverseFunctionalProperty"),
        has_value: owl("hasValue"),
        on_property: owl("onProperty"),
        some_values: owl("someValuesFrom"),
        all_values: owl("allValuesFrom"),
        intersection: owl("intersectionOf"),
        union: owl("unionOf"),
        chain: owl("propertyChainAxiom"),
        first: named_node_id(rdf::FIRST.as_str()),
        rest: named_node_id(rdf::REST.as_str()),
        nil: named_node_id(rdf::NIL.as_str()),
        thing: owl("Thing"),
    }
}

/// `tbox_closure.kind` values.
pub mod kind {
    /// `sub ⊑ sup` for classes (subClassOf and equivalentClass, transitive, irreflexive rows).
    pub const CLASS: i64 = 1;
    /// RDFS sub-property closure (subPropertyOf and equivalentProperty).
    pub const PROPERTY: i64 = 2;
    /// OWL: `(x sub y)` entails `(x sup y)` (sub-properties, inverses of inverses).
    pub const OWL_SAME: i64 = 3;
    /// OWL: `(x sub y)` entails `(y sup x)` (inverse and symmetric properties).
    pub const OWL_INVERSE: i64 = 4;
    /// Transitive properties (`sub = sup`).
    pub const TRANSITIVE: i64 = 5;
    /// RDFS: the subject of `sub` has type `sup` (domains through sub-properties/classes).
    pub const SUBJECT_TYPE: i64 = 6;
    /// RDFS: the object of `sub` has type `sup`.
    pub const OBJECT_TYPE: i64 = 7;
    /// OWL: the subject of `sub` has type `sup` (also through inverse ranges).
    pub const OWL_SUBJECT_TYPE: i64 = 8;
    /// OWL: the object of `sub` has type `sup`.
    pub const OWL_OBJECT_TYPE: i64 = 9;
}

/// Predicates whose triples change the schema closure.
pub fn schema_predicates() -> [i64; 6] {
    let v = vocab();
    [v.sco, v.spo, v.dom, v.rng, v.eqc, v.eqp]
}

/// Does a triple with this predicate and object change the closure?
pub fn is_schema_triple(p: i64, o: i64) -> bool {
    let v = vocab();
    schema_predicates().contains(&p) || p == v.inv || (p == v.ty && (o == v.sym || o == v.trans))
}

fn is_schema_iri(p: &str) -> bool {
    p == rdfs::SUB_CLASS_OF.as_str()
        || p == rdfs::SUB_PROPERTY_OF.as_str()
        || p == rdfs::DOMAIN.as_str()
        || p == rdfs::RANGE.as_str()
        || p.strip_prefix(OWL)
            .is_some_and(|l| matches!(l, "equivalentClass" | "equivalentProperty" | "inverseOf"))
}

fn is_axiom_class(o: &str) -> bool {
    o.strip_prefix(OWL)
        .is_some_and(|l| matches!(l, "SymmetricProperty" | "TransitiveProperty"))
}

/// Does writing this quad change the schema closure? (A schema axiom, or anything in the
/// registry graph, which scopes the closure.)
pub fn is_schema_quad(q: QuadRef<'_>) -> bool {
    crate::registry::is_registry_quad(q)
        || is_schema_iri(q.predicate.as_str())
        || (q.predicate == rdf::TYPE
            && matches!(q.object, TermRef::NamedNode(n) if is_axiom_class(n.as_str())))
}

/// Can this update change the schema closure? (Conservative: variables count as schema.)
pub fn update_touches_schema(update: &Update) -> bool {
    crate::registry::update_touches_registry(update) || touches_axioms(update)
}

fn touches_axioms(update: &Update) -> bool {
    let pattern = |p: &NamedNodePattern, o: &TermPattern| match p {
        NamedNodePattern::Variable(_) => true,
        NamedNodePattern::NamedNode(n) if is_schema_iri(n.as_str()) => true,
        NamedNodePattern::NamedNode(n) if *n == rdf::TYPE => match o {
            TermPattern::NamedNode(c) => is_axiom_class(c.as_str()),
            TermPattern::Variable(_) => true,
            _ => false,
        },
        NamedNodePattern::NamedNode(_) => false,
    };
    update.operations.iter().any(|op| match op {
        GraphUpdateOperation::InsertData { data } => data.iter().any(|q| {
            is_schema_iri(q.predicate.as_str())
                || (q.predicate == rdf::TYPE && matches!(&q.object, Term::NamedNode(n) if is_axiom_class(n.as_str())))
        }),
        GraphUpdateOperation::DeleteData { data } => data.iter().any(|q| {
            is_schema_iri(q.predicate.as_str())
                || (q.predicate == rdf::TYPE
                    && matches!(&q.object, spargebra::term::GroundTerm::NamedNode(n) if is_axiom_class(n.as_str())))
        }),
        GraphUpdateOperation::DeleteInsert { delete, insert, .. } => {
            delete.iter().any(|q| {
                let o: TermPattern = q.object.clone().into();
                pattern(&q.predicate, &o)
            }) || insert.iter().any(|q| pattern(&q.predicate, &q.object))
        }
        GraphUpdateOperation::Create { .. } => false,
        GraphUpdateOperation::Load { .. }
        | GraphUpdateOperation::Clear { .. }
        | GraphUpdateOperation::Drop { .. } => true,
    })
}

/// SQL: the id is an IRI, a blank node or a triple term (a valid subject).
pub(crate) fn non_literal(x: &str) -> String {
    format!("(({x}) >> {PAYLOAD_BITS}) IN (1, 2, 9)")
}

/// Statements recomputing `tbox_closure`, one closure per scope (see
/// [`crate::registry::ontology_axioms`]): the scope of every graph, and one per graph that an
/// active ontology is mapped to. Each statement reads the ontology axioms tagged with their
/// scope; recursion only joins rows of the same scope.
pub fn closure_statements() -> Vec<Statement> {
    let v = vocab();
    let ax = |cond: String| crate::registry::ontology_axioms(&cond);
    let (c, p, s_, t) = (
        kind::CLASS,
        kind::PROPERTY,
        kind::OWL_SAME,
        kind::TRANSITIVE,
    );
    let class_edges = format!(
        "SELECT scope AS k, s AS a, o AS b FROM {} UNION SELECT scope, o, s FROM {}",
        ax(format!("q.p IN ({}, {})", v.sco, v.eqc)),
        ax(format!("q.p = {}", v.eqc))
    );
    let prop_edges = format!(
        "SELECT scope AS k, s AS a, o AS b FROM {} UNION SELECT scope, o, s FROM {}",
        ax(format!("q.p IN ({}, {})", v.spo, v.eqp)),
        ax(format!("q.p = {}", v.eqp))
    );
    let closure = |k: i64, edges: &str| {
        Statement::new(format!(
            "WITH RECURSIVE e(k, a, b) AS ({edges}), \
             c(k, a, b) AS (SELECT k, a, b FROM e UNION SELECT c.k, c.a, e.b FROM c JOIN e ON e.k = c.k AND e.a = c.b) \
             INSERT OR IGNORE INTO tbox_closure(kind, scope, sub, sup) SELECT {k}, k, a, b FROM c WHERE a <> b"
        ))
    };
    // OWL: property edges with a direction bit (1 = inverse), composed modulo 2.
    let owl = Statement::new(format!(
        "WITH RECURSIVE e(k, a, b, d) AS (SELECT k, a, b, 0 FROM ({prop_edges}) \
           UNION SELECT scope, s, o, 1 FROM {inv} UNION SELECT scope, o, s, 1 FROM {inv} \
           UNION SELECT scope, s, s, 1 FROM {sym}), \
         c(k, a, b, d) AS (SELECT k, a, b, d FROM e UNION SELECT c.k, c.a, e.b, (c.d + e.d) % 2 FROM c JOIN e ON e.k = c.k AND e.a = c.b) \
         INSERT OR IGNORE INTO tbox_closure(kind, scope, sub, sup) SELECT {s_} + d, k, a, b FROM c WHERE d = 1 OR a <> b",
        inv = ax(format!("q.p = {}", v.inv)),
        sym = ax(format!("q.p = {} AND q.o = {}", v.ty, v.sym)),
    ));
    let dom_rng = || ax(format!("q.p IN ({}, {})", v.dom, v.rng));
    // Classes a domain/range class is a subclass of (reflexive), per scope.
    let classes = format!(
        "SELECT scope AS k, o AS a, o AS b FROM {} UNION SELECT scope, sub, sup FROM tbox_closure WHERE kind = {c}",
        dom_rng()
    );
    // Properties with (sub, sup) where sup is reflexive over properties having a domain/range.
    let subs = |k: i64| {
        format!(
            "SELECT scope AS k, s AS a, s AS b FROM {} UNION SELECT scope, sub, sup FROM tbox_closure WHERE kind = {k}",
            dom_rng()
        )
    };
    let typed = |k: i64, props: &str, axiom: i64| {
        format!(
            "SELECT {k}, pq.k, pq.a, cd.b FROM ({props}) pq JOIN {} x ON x.s = pq.b AND x.scope = pq.k \
             JOIN ({classes}) cd ON cd.a = x.o AND cd.k = pq.k",
            ax(format!("q.p = {axiom}"))
        )
    };
    let inverse_typed = |k: i64, axiom: i64| {
        format!(
            "SELECT {k}, pq.scope, pq.sub, cd.b FROM tbox_closure pq JOIN {} x ON x.s = pq.sup AND x.scope = pq.scope \
             JOIN ({classes}) cd ON cd.a = x.o AND cd.k = pq.scope WHERE pq.kind = {inv}",
            ax(format!("q.p = {axiom}")),
            inv = kind::OWL_INVERSE
        )
    };
    let insert = |sql: String| {
        Statement::new(format!(
            "INSERT OR IGNORE INTO tbox_closure(kind, scope, sub, sup) {sql}"
        ))
    };
    vec![
        Statement::new("DELETE FROM tbox_closure"),
        closure(c, &class_edges),
        closure(p, &prop_edges),
        owl,
        Statement::new(format!(
            "INSERT OR IGNORE INTO tbox_closure(kind, scope, sub, sup) SELECT {t}, scope, s, s FROM {}",
            ax(format!("q.p = {} AND q.o = {}", v.ty, v.trans))
        )),
        insert(typed(kind::SUBJECT_TYPE, &subs(p), v.dom)),
        insert(typed(kind::OBJECT_TYPE, &subs(p), v.rng)),
        insert(format!(
            "{} UNION {}",
            typed(kind::OWL_SUBJECT_TYPE, &subs(s_), v.dom),
            inverse_typed(kind::OWL_SUBJECT_TYPE, v.rng)
        )),
        insert(format!(
            "{} UNION {}",
            typed(kind::OWL_OBJECT_TYPE, &subs(s_), v.rng),
            inverse_typed(kind::OWL_OBJECT_TYPE, v.dom)
        )),
    ]
}

/// Loads the transitive properties (kept in memory with the planner statistics).
pub fn transitive_statement(id_col: impl Fn(&str) -> String) -> Statement {
    Statement::new(format!(
        "SELECT DISTINCT {} FROM tbox_closure WHERE kind = {}",
        id_col("sub"),
        kind::TRANSITIVE
    ))
}

/// Which graphs a reasoned pattern reads.
#[derive(Debug, Clone)]
pub enum GraphFilter {
    /// Keep each quad's graph (the caller constrains `g`).
    Keep,
    /// The RDF merge of these graphs (`None`: every graph), exposed as graph 0.
    Merge(Option<Vec<i64>>),
}

/// Builds derived tables of entailed triples.
#[derive(Debug, Clone)]
pub struct Entailment<'a> {
    pub reasoning: Reasoning,
    /// Also read materialized inferences (`quads_inf`).
    pub inferred: bool,
    /// Exclude every registered schema graph from the stored triples
    /// (`QueryOptions::include_schema_graphs`).
    pub hide_schema: bool,
    pub transitive: &'a BTreeSet<i64>,
    /// Graphs with a closure of their own (see `tbox_closure.scope`); every other graph uses
    /// the closure of [`crate::registry::all_scope`].
    pub scopes: &'a BTreeSet<i64>,
    /// Maximum terms of a compound SELECT.
    pub max_compound: usize,
    /// Read the store as it was at this tick (see `version`), from the change log.
    pub as_of: Option<i64>,
}

impl Entailment<'_> {
    /// SQL: the closure scope of the quads whose graph column is `g`.
    fn scope_of(&self, g: &str) -> String {
        let all = crate::registry::all_scope();
        if self.scopes.is_empty() {
            return all.to_string();
        }
        let list = self
            .scopes
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!("(CASE WHEN {g} IN ({list}) THEN {g} ELSE {all} END)")
    }

    /// Is any rewriting needed?
    pub fn active(&self) -> bool {
        self.reasoning != Reasoning::None || self.inferred
    }

    /// The table of stored triples: asserted, plus materialized inferences on request, minus
    /// the registered schema graphs when they are hidden.
    ///
    /// Every quad source of the compiler goes through this or [`Self::source`], so hiding is
    /// applied once and covers patterns, paths, `OPTIONAL` and `GRAPH ?g` alike. Inferences
    /// are conclusions rather than schema, so they are never hidden.
    pub fn base(&self) -> String {
        let asserted = match (self.as_of, self.hide_schema) {
            (Some(t), true) => {
                crate::registry::without_schema_graphs(&crate::version::as_of_sql(&t.to_string()))
            }
            (Some(t), false) => crate::version::as_of_sql(&t.to_string()),
            (None, true) => crate::registry::quads_without_schema_graphs().to_owned(),
            (None, false) => "quads".to_owned(),
        };
        if self.inferred {
            format!(
                "(SELECT s, p, o, g FROM {asserted} UNION ALL SELECT s, p, o, g FROM quads_inf)"
            )
        } else {
            asserted.to_string()
        }
    }

    fn kinds(&self) -> (i64, Option<i64>, i64, i64) {
        match self.reasoning {
            Reasoning::OwlQl => (
                kind::OWL_SAME,
                Some(kind::OWL_INVERSE),
                kind::OWL_SUBJECT_TYPE,
                kind::OWL_OBJECT_TYPE,
            ),
            _ => (kind::PROPERTY, None, kind::SUBJECT_TYPE, kind::OBJECT_TYPE),
        }
    }

    /// A `(s, p, o, g)` derived table of the entailed triples matching the constants.
    pub fn source(
        &self,
        s: Option<i64>,
        p: Option<i64>,
        o: Option<i64>,
        graphs: &GraphFilter,
    ) -> String {
        let base = self.base();
        let g = |x: &str| match graphs {
            GraphFilter::Keep => format!("{x}.g"),
            GraphFilter::Merge(_) => DEFAULT_GRAPH_ID.to_string(),
        };
        let gw = |x: &str| match graphs {
            GraphFilter::Merge(Some(l)) => format!(
                " AND {x}.g IN ({})",
                l.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")
            ),
            _ => String::new(),
        };
        let eq =
            |col: &str, v: Option<i64>| v.map(|v| format!(" AND {col} = {v}")).unwrap_or_default();
        if self.reasoning == Reasoning::None {
            let mut w = String::from("1");
            w.push_str(&eq("x.s", s));
            w.push_str(&eq("x.p", p));
            w.push_str(&eq("x.o", o));
            w.push_str(&gw("x"));
            let d = if matches!(graphs, GraphFilter::Merge(_)) {
                "DISTINCT "
            } else {
                ""
            };
            return format!(
                "(SELECT {d}x.s AS s, x.p AS p, x.o AS o, {} AS g FROM {base} x WHERE {w})",
                g("x")
            );
        }
        let v = vocab();
        let (same, inv, subj, obj) = self.kinds();
        // Each quad is entailed with the closure of its graph's scope.
        let cs = format!(" AND c.scope = {}", self.scope_of("x.g"));
        let cs = cs.as_str();
        // One arm: `SELECT s AS s, p AS p, o AS o, g AS g FROM …` (compound SELECTs take their
        // column names from the first arm, and derived tables cannot rename columns).
        let row = |s: &str, p: &str, o: &str, gx: &str, rest: String| {
            format!(
                "SELECT {s} AS s, {p} AS p, {o} AS o, {} AS g FROM {rest}",
                g(gx)
            )
        };
        let ty = v.ty.to_string();
        let type_arms = |s: Option<i64>, o: Option<i64>, asserted: bool| -> Vec<String> {
            let mut a = Vec::new();
            if asserted {
                a.push(row(
                    "x.s",
                    &ty,
                    "x.o",
                    "x",
                    format!(
                        "{base} x WHERE x.p = {ty}{}{}{}",
                        eq("x.s", s),
                        eq("x.o", o),
                        gw("x")
                    ),
                ));
            }
            // Superclasses of asserted types, then domains and ranges.
            a.push(row("x.s", &ty, "c.sup", "x", format!(
                "{base} x JOIN tbox_closure c ON c.kind = {} AND c.sub = x.o WHERE x.p = {ty}{cs}{}{}{}",
                kind::CLASS, eq("x.s", s), eq("c.sup", o), gw("x")
            )));
            a.push(row(
                "x.s",
                &ty,
                "c.sup",
                "x",
                format!(
                    "tbox_closure c JOIN {base} x ON x.p = c.sub WHERE c.kind = {subj}{cs}{}{}{}",
                    eq("x.s", s),
                    eq("c.sup", o),
                    gw("x")
                ),
            ));
            a.push(row(
                "x.o",
                &ty,
                "c.sup",
                "x",
                format!(
                    "tbox_closure c JOIN {base} x ON x.p = c.sub WHERE c.kind = {obj}{cs} AND {}{}{}{}",
                    non_literal("x.o"),
                    eq("x.o", s),
                    eq("c.sup", o),
                    gw("x")
                ),
            ));
            a
        };
        // Triples of property `pid` entailed without transitivity.
        let prop_arms = |pid: i64, s: Option<i64>, o: Option<i64>| -> Vec<String> {
            let pid_s = pid.to_string();
            let mut a = vec![
                row("x.s", &pid_s, "x.o", "x", format!("{base} x WHERE x.p = {pid}{}{}{}", eq("x.s", s), eq("x.o", o), gw("x"))),
                row("x.s", &pid_s, "x.o", "x", format!(
                    "tbox_closure c JOIN {base} x ON x.p = c.sub WHERE c.kind = {same} AND c.sup = {pid}{cs}{}{}{}",
                    eq("x.s", s), eq("x.o", o), gw("x")
                )),
            ];
            if let Some(inv) = inv {
                a.push(row("x.o", &pid_s, "x.s", "x", format!(
                    "tbox_closure c JOIN {base} x ON x.p = c.sub WHERE c.kind = {inv} AND c.sup = {pid}{cs} AND {}{}{}{}",
                    non_literal("x.o"), eq("x.o", s), eq("x.s", o), gw("x")
                )));
            }
            // The schema closure itself answers subClassOf / subPropertyOf patterns.
            let schema_kind = if pid == v.sco {
                Some(kind::CLASS)
            } else if pid == v.spo {
                Some(kind::PROPERTY)
            } else {
                None
            };
            if let Some(k) = schema_kind {
                a.push(format!(
                    "SELECT sub AS s, {pid} AS p, sup AS o, {DEFAULT_GRAPH_ID} AS g FROM tbox_closure WHERE kind = {k}{}{}",
                    eq("sub", s),
                    eq("sup", o)
                ));
            }
            a
        };
        // A transitive property: the closure of its (otherwise) entailed triples, walked from a
        // constant endpoint when there is one.
        // With graphs of their own scope, a property is transitive only in the graphs whose
        // scope declares it so; elsewhere its triples are entailed as for any property.
        let declared = |pid: i64, g: &str| {
            format!(
                "EXISTS (SELECT 1 FROM tbox_closure t WHERE t.kind = {} AND t.sub = {pid} AND t.scope = {})",
                kind::TRANSITIVE,
                self.scope_of(g)
            )
        };
        let transitive = |pid: i64, s: Option<i64>, o: Option<i64>| -> Vec<String> {
            let mut u = union_all(prop_arms(pid, None, None), self.max_compound);
            let mut arms = Vec::new();
            if !self.scopes.is_empty() {
                u = format!(
                    "SELECT z.s, z.p, z.o, z.g FROM ({u}) z WHERE {}",
                    declared(pid, "z.g")
                );
                arms.push(format!(
                    "SELECT z.s, z.p, z.o, z.g FROM ({}) z WHERE NOT {}",
                    union_all(prop_arms(pid, s, o), self.max_compound),
                    declared(pid, "z.g")
                ));
            }
            let rec = match (s, o) {
                (Some(s), _) => format!(
                    "r(n, g) AS (SELECT o, g FROM u WHERE s = {s} UNION SELECT u.o, u.g FROM r JOIN u ON u.s = r.n AND u.g = r.g) \
                     SELECT {s} AS s, {pid} AS p, n AS o, g FROM r{}",
                    o.map(|o| format!(" WHERE n = {o}")).unwrap_or_default()
                ),
                (None, Some(o)) => format!(
                    "r(n, g) AS (SELECT s, g FROM u WHERE o = {o} UNION SELECT u.s, u.g FROM r JOIN u ON u.o = r.n AND u.g = r.g) \
                     SELECT n AS s, {pid} AS p, {o} AS o, g FROM r"
                ),
                (None, None) => format!(
                    "r(s, o, g) AS (SELECT s, o, g FROM u UNION SELECT r.s, u.o, r.g FROM r JOIN u ON u.s = r.o AND u.g = r.g) \
                     SELECT s, {pid} AS p, o, g FROM r"
                ),
            };
            arms.push(format!(
                "SELECT * FROM (WITH RECURSIVE u(s, p, o, g) AS ({u}), {rec})"
            ));
            arms
        };
        let mut arms = Vec::new();
        match p {
            Some(pid) if pid == v.ty => arms.extend(type_arms(s, o, true)),
            Some(pid) if self.reasoning == Reasoning::OwlQl && self.transitive.contains(&pid) => {
                arms.extend(transitive(pid, s, o));
            }
            Some(pid) => arms.extend(prop_arms(pid, s, o)),
            None => {
                // Every entailed triple: asserted, through property axioms, the schema closure,
                // types, and transitive properties.
                arms.push(row(
                    "x.s",
                    "x.p",
                    "x.o",
                    "x",
                    format!(
                        "{base} x WHERE 1{}{}{}",
                        eq("x.s", s),
                        eq("x.o", o),
                        gw("x")
                    ),
                ));
                arms.push(row(
                    "x.s",
                    "c.sup",
                    "x.o",
                    "x",
                    format!(
                        "tbox_closure c JOIN {base} x ON x.p = c.sub WHERE c.kind = {same}{cs}{}{}{}",
                        eq("x.s", s),
                        eq("x.o", o),
                        gw("x")
                    ),
                ));
                if let Some(inv) = inv {
                    arms.push(row("x.o", "c.sup", "x.s", "x", format!(
                        "tbox_closure c JOIN {base} x ON x.p = c.sub WHERE c.kind = {inv}{cs} AND {}{}{}{}",
                        non_literal("x.o"), eq("x.o", s), eq("x.s", o), gw("x")
                    )));
                }
                for (k, pid) in [(kind::CLASS, v.sco), (kind::PROPERTY, v.spo)] {
                    arms.push(format!(
                        "SELECT sub AS s, {pid} AS p, sup AS o, {DEFAULT_GRAPH_ID} AS g FROM tbox_closure WHERE kind = {k}{}{}",
                        eq("sub", s),
                        eq("sup", o)
                    ));
                }
                arms.extend(type_arms(s, o, false));
                if self.reasoning == Reasoning::OwlQl {
                    for &pid in self.transitive {
                        arms.extend(transitive(pid, s, o));
                    }
                }
            }
        }
        format!(
            "(SELECT DISTINCT s, p, o, g FROM ({}))",
            union_all(arms, self.max_compound)
        )
    }
}

/// Statements of one OWL 2 RL round: every rule inserts its new conclusions into
/// `quads_inf` (graph 0), reading asserted and inferred triples of every graph.
pub fn materialize_round() -> Vec<Statement> {
    let v = vocab();
    let a = "(SELECT s, p, o FROM quads UNION ALL SELECT s, p, o FROM quads_inf)";
    let nl = |x: &str| non_literal(x);
    let lists = format!(
        "RECURSIVE l(head, node, item) AS (SELECT s, s, o FROM {a} WHERE p = {first} \
           UNION SELECT l.head, r.o, f.o FROM l JOIN {a} r ON r.s = l.node AND r.p = {rest} JOIN {a} f ON f.s = r.o AND f.p = {first})",
        first = v.first,
        rest = v.rest
    );
    let rules: Vec<String> = vec![
        // eq-sym, eq-trans
        format!("SELECT o, {same}, s FROM {a} WHERE p = {same}", same = v.same),
        format!("SELECT x.s, {same}, y.o FROM {a} x JOIN {a} y ON y.s = x.o AND y.p = {same} WHERE x.p = {same}", same = v.same),
        // eq-rep-s, eq-rep-p, eq-rep-o
        format!("SELECT e.o, t.p, t.o FROM {a} e JOIN {a} t ON t.s = e.s WHERE e.p = {same} AND {}", nl("e.o"), same = v.same),
        format!("SELECT t.s, e.o, t.o FROM {a} e JOIN {a} t ON t.p = e.s WHERE e.p = {same} AND ((e.o) >> {PAYLOAD_BITS}) = 1", same = v.same),
        format!("SELECT t.s, t.p, e.o FROM {a} e JOIN {a} t ON t.o = e.s WHERE e.p = {same}", same = v.same),
        // prp-dom, prp-rng
        format!("SELECT t.s, {ty}, d.o FROM {a} d JOIN {a} t ON t.p = d.s WHERE d.p = {dom}", ty = v.ty, dom = v.dom),
        format!("SELECT t.o, {ty}, d.o FROM {a} d JOIN {a} t ON t.p = d.s WHERE d.p = {rng} AND {}", nl("t.o"), ty = v.ty, rng = v.rng),
        // prp-fp, prp-ifp
        format!(
            "SELECT t1.o, {same}, t2.o FROM {a} f JOIN {a} t1 ON t1.p = f.s JOIN {a} t2 ON t2.p = f.s AND t2.s = t1.s \
             WHERE f.p = {ty} AND f.o = {func} AND t1.o <> t2.o AND {} AND {}",
            nl("t1.o"), nl("t2.o"), same = v.same, ty = v.ty, func = v.func
        ),
        format!(
            "SELECT t1.s, {same}, t2.s FROM {a} f JOIN {a} t1 ON t1.p = f.s JOIN {a} t2 ON t2.p = f.s AND t2.o = t1.o \
             WHERE f.p = {ty} AND f.o = {ifunc} AND t1.s <> t2.s",
            same = v.same, ty = v.ty, ifunc = v.ifunc
        ),
        // prp-symp, prp-trp
        format!("SELECT t.o, t.p, t.s FROM {a} f JOIN {a} t ON t.p = f.s WHERE f.p = {ty} AND f.o = {sym} AND {}", nl("t.o"), ty = v.ty, sym = v.sym),
        format!(
            "SELECT t1.s, t1.p, t2.o FROM {a} f JOIN {a} t1 ON t1.p = f.s JOIN {a} t2 ON t2.p = f.s AND t2.s = t1.o WHERE f.p = {ty} AND f.o = {trans}",
            ty = v.ty, trans = v.trans
        ),
        // prp-spo1, prp-eqp1, prp-eqp2
        format!("SELECT t.s, x.o, t.o FROM {a} x JOIN {a} t ON t.p = x.s WHERE x.p = {spo}", spo = v.spo),
        format!("SELECT t.s, x.o, t.o FROM {a} x JOIN {a} t ON t.p = x.s WHERE x.p = {eqp}", eqp = v.eqp),
        format!("SELECT t.s, x.s, t.o FROM {a} x JOIN {a} t ON t.p = x.o WHERE x.p = {eqp}", eqp = v.eqp),
        // prp-inv1, prp-inv2
        format!("SELECT t.o, x.o, t.s FROM {a} x JOIN {a} t ON t.p = x.s WHERE x.p = {inv} AND {}", nl("t.o"), inv = v.inv),
        format!("SELECT t.o, x.s, t.s FROM {a} x JOIN {a} t ON t.p = x.o WHERE x.p = {inv} AND {}", nl("t.o"), inv = v.inv),
        // cax-sco, cax-eqc1, cax-eqc2
        format!("SELECT t.s, {ty}, x.o FROM {a} x JOIN {a} t ON t.p = {ty} AND t.o = x.s WHERE x.p = {sco}", ty = v.ty, sco = v.sco),
        format!("SELECT t.s, {ty}, x.o FROM {a} x JOIN {a} t ON t.p = {ty} AND t.o = x.s WHERE x.p = {eqc}", ty = v.ty, eqc = v.eqc),
        format!("SELECT t.s, {ty}, x.s FROM {a} x JOIN {a} t ON t.p = {ty} AND t.o = x.o WHERE x.p = {eqc}", ty = v.ty, eqc = v.eqc),
        // scm-sco (transitive subclasses)
        format!("SELECT x.s, {sco}, y.o FROM {a} x JOIN {a} y ON y.s = x.o AND y.p = {sco} WHERE x.p = {sco}", sco = v.sco),
        // scm-eqc1 (the other scm-* rules only restate schema triples whose instance-level
        // consequences the rules above already derive; `reasonable` omits them too)
        format!("SELECT s, {sco}, o FROM {a} WHERE p = {eqc} UNION ALL SELECT o, {sco}, s FROM {a} WHERE p = {eqc}", sco = v.sco, eqc = v.eqc),
        // Every subject is an owl:Thing; owl:Thing and owl:Nothing are classes (cls-thing, cls-nothing1)
        format!("SELECT s, {ty}, {thing} FROM {a} WHERE {} UNION SELECT {thing}, {ty}, {class} UNION SELECT {nothing}, {ty}, {class}", nl("s"), ty = v.ty, thing = v.thing, class = owl("Class"), nothing = owl("Nothing")),
        // cls-hv1, cls-hv2
        format!(
            "SELECT t.s, op.o, hv.o FROM {a} hv JOIN {a} op ON op.s = hv.s AND op.p = {onp} JOIN {a} t ON t.p = {ty} AND t.o = hv.s WHERE hv.p = {hv}",
            onp = v.on_property, ty = v.ty, hv = v.has_value
        ),
        format!(
            "SELECT t.s, {ty}, hv.s FROM {a} hv JOIN {a} op ON op.s = hv.s AND op.p = {onp} JOIN {a} t ON t.p = op.o AND t.o = hv.o WHERE hv.p = {hv}",
            onp = v.on_property, ty = v.ty, hv = v.has_value
        ),
        // cls-svf1, cls-svf2
        format!(
            "SELECT t.s, {ty}, sv.s FROM {a} sv JOIN {a} op ON op.s = sv.s AND op.p = {onp} JOIN {a} t ON t.p = op.o \
             JOIN {a} vt ON vt.s = t.o AND vt.p = {ty} AND vt.o = sv.o WHERE sv.p = {svf}",
            onp = v.on_property, ty = v.ty, svf = v.some_values
        ),
        format!(
            "SELECT t.s, {ty}, sv.s FROM {a} sv JOIN {a} op ON op.s = sv.s AND op.p = {onp} JOIN {a} t ON t.p = op.o WHERE sv.p = {svf} AND sv.o = {thing}",
            onp = v.on_property, ty = v.ty, svf = v.some_values, thing = v.thing
        ),
        // cls-avf
        format!(
            "SELECT t.o, {ty}, av.o FROM {a} av JOIN {a} op ON op.s = av.s AND op.p = {onp} JOIN {a} xt ON xt.p = {ty} AND xt.o = av.s \
             JOIN {a} t ON t.s = xt.s AND t.p = op.o WHERE av.p = {avf} AND {}",
            nl("t.o"), onp = v.on_property, ty = v.ty, avf = v.all_values
        ),
        // cls-int1, cls-int2, cls-uni (list members through rdf:first / rdf:rest)
        format!(
            "WITH {lists}, m(c, item) AS (SELECT x.s, l.item FROM {a} x JOIN l ON l.head = x.o WHERE x.p = {int}) \
             SELECT t.s, {ty}, m.c FROM m JOIN {a} t ON t.p = {ty} AND t.o = m.item GROUP BY m.c, t.s \
             HAVING COUNT(DISTINCT m.item) = (SELECT COUNT(DISTINCT m2.item) FROM m m2 WHERE m2.c = m.c)",
            ty = v.ty, int = v.intersection
        ),
        format!(
            "WITH {lists} SELECT t.s, {ty}, l.item FROM {a} x JOIN l ON l.head = x.o JOIN {a} t ON t.p = {ty} AND t.o = x.s WHERE x.p = {int}",
            ty = v.ty, int = v.intersection
        ),
        format!(
            "WITH {lists} SELECT t.s, {ty}, x.s FROM {a} x JOIN l ON l.head = x.o JOIN {a} t ON t.p = {ty} AND t.o = l.item WHERE x.p = {uni}",
            ty = v.ty, uni = v.union
        ),
        // prp-spo2 (property chains)
        format!(
            "WITH {lists}, \
               ix(head, node, i) AS (SELECT s, s, 1 FROM {a} WHERE p = {first} UNION SELECT ix.head, r.o, ix.i + 1 FROM ix JOIN {a} r ON r.s = ix.node AND r.p = {rest} WHERE r.o <> {nil}), \
               steps(chain, i, prop) AS (SELECT x.s, ix.i, f.o FROM {a} x JOIN ix ON ix.head = x.o JOIN {a} f ON f.s = ix.node AND f.p = {first} WHERE x.p = {chain}), \
               w(chain, start, i, cur) AS (SELECT st.chain, t.s, 1, t.o FROM steps st JOIN {a} t ON t.p = st.prop WHERE st.i = 1 \
                 UNION SELECT w.chain, w.start, w.i + 1, t.o FROM w JOIN steps st ON st.chain = w.chain AND st.i = w.i + 1 JOIN {a} t ON t.s = w.cur AND t.p = st.prop) \
             SELECT w.start, w.chain, w.cur FROM w WHERE w.i = (SELECT MAX(i) FROM steps s2 WHERE s2.chain = w.chain)",
            first = v.first, rest = v.rest, nil = v.nil, chain = v.chain
        ),
    ];
    let mut statements: Vec<Statement> = rules
        .into_iter()
        .map(|r| {
            let (with, body) = match r.strip_prefix("WITH ") {
                Some(rest) => split_with(rest),
                None => (String::new(), r),
            };
            let body = name_columns(&body);
            Statement::new(format!(
                "{with}INSERT OR IGNORE INTO quads_inf(s, p, o, g) SELECT DISTINCT r.s, r.p, r.o, {DEFAULT_GRAPH_ID} FROM ({body}) AS r \
                 WHERE {} AND NOT EXISTS (SELECT 1 FROM quads a WHERE a.s = r.s AND a.p = r.p AND a.o = r.o AND a.g = {DEFAULT_GRAPH_ID})",
                non_literal("r.s")
            ))
        })
        .collect();
    statements.push(inference_attribute(OWL_PRODUCER));
    statements
}

/// Renames the three result columns of a rule's first SELECT to `s, p, o`.
fn name_columns(select: &str) -> String {
    let rest = select.strip_prefix("SELECT ").unwrap_or(select);
    let from = top_level(rest, " FROM ").unwrap_or(rest.len());
    let cols: Vec<&str> = split_top(&rest[..from], ',');
    if cols.len() != 3 {
        return select.to_string();
    }
    format!(
        "SELECT {} AS s, {} AS p, {} AS o{}",
        cols[0].trim(),
        cols[1].trim(),
        cols[2].trim(),
        &rest[from..]
    )
}

/// Byte offset of the first top-level (outside parentheses) occurrence of `pat`.
fn top_level(s: &str, pat: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ if depth == 0 && s[i..].starts_with(pat) => return Some(i),
            _ => {}
        }
    }
    None
}

fn split_top(s: &str, sep: char) -> Vec<&str> {
    let (mut depth, mut start, mut out) = (0i32, 0usize, Vec::new());
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Splits `ctes SELECT …` (after `WITH `) into the `WITH … ` prefix and the final SELECT.
fn split_with(rest: &str) -> (String, String) {
    // The final SELECT is the last top-level "SELECT" (outside parentheses).
    let bytes = rest.as_bytes();
    let (mut depth, mut last) = (0i32, 0usize);
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'S' if depth == 0 && rest[i..].starts_with("SELECT ") => last = i,
            _ => {}
        }
    }
    (
        format!("WITH {} ", rest[..last].trim_end()),
        rest[last..].to_string(),
    )
}

/// The producer id of OWL 2 RL materialization (SQL rules and `reasonable` alike).
pub const OWL_PRODUCER: &str = "owl2rl";

/// The id a producer name is stored under in `quads_inf_src` and `inf_producers`.
pub fn producer_id(name: &str) -> i64 {
    // A positive 62-bit hash: stable across runs and backends, computed without a lookup.
    (xxhash_rust::xxh3::xxh3_64(name.as_bytes()) >> 2) as i64
}

/// Discards one producer's inferences: its attributions go, and so does every inferred quad no
/// other producer derived. The producer is (re)registered under its name.
pub fn inference_reset(name: &str) -> Vec<Statement> {
    let id = producer_id(name);
    vec![
        Statement::new(format!(
            "INSERT OR REPLACE INTO inf_producers(id, name) VALUES ({id}, '{}')",
            name.replace('\'', "''")
        )),
        Statement::new(format!("DELETE FROM quads_inf_src WHERE src = {id}")),
        Statement::new(
            "DELETE FROM quads_inf WHERE NOT EXISTS (SELECT 1 FROM quads_inf_src q \
             WHERE q.s = quads_inf.s AND q.p = quads_inf.p AND q.o = quads_inf.o AND q.g = quads_inf.g)",
        ),
    ]
}

/// Attributes every inferred quad no producer claims yet to `name`. Run after a producer
/// writes, so what it derived (and nobody had derived before) is recorded as its own.
pub fn inference_attribute(name: &str) -> Statement {
    Statement::new(format!(
        "INSERT OR IGNORE INTO quads_inf_src(src, s, p, o, g) SELECT {}, i.s, i.p, i.o, i.g FROM quads_inf i \
         WHERE NOT EXISTS (SELECT 1 FROM quads_inf_src q WHERE q.s = i.s AND q.p = i.p AND q.o = i.o AND q.g = i.g)",
        producer_id(name)
    ))
}

/// Statements run before OWL 2 RL materialization (see [`materialize_reset_for`]).
pub fn materialize_reset(caps: &crate::sql::Capabilities) -> Vec<Statement> {
    materialize_reset_for(caps, OWL_PRODUCER)
}

/// Statements run before a producer materializes: its previous inferences are discarded, and
/// the vocabulary that rule conclusions use (`owl:sameAs`, `rdfs:subClassOf`, …) gets its term
/// rows, since it may not appear in the data.
pub fn materialize_reset_for(caps: &crate::sql::Capabilities, producer: &str) -> Vec<Statement> {
    let mut rows = crate::encoding::EncodedRows::default();
    for iri in [
        rdf::TYPE.as_str(),
        rdfs::SUB_CLASS_OF.as_str(),
        rdfs::SUB_PROPERTY_OF.as_str(),
        rdfs::DOMAIN.as_str(),
        rdfs::RANGE.as_str(),
    ] {
        rows.iri(iri);
    }
    for local in [
        "sameAs",
        "equivalentClass",
        "equivalentProperty",
        "Thing",
        "Nothing",
        "Class",
    ] {
        rows.iri(&format!("{OWL}{local}"));
    }
    rows.dedup();
    let mut out = inference_reset(producer);
    out.extend(crate::writer::term_statements(&rows, caps));
    out
}

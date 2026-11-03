# oxilite-elpi

λProlog programs over [oxilite](https://github.com/Volland/oxilite), run by
[lean-elpi](https://github.com/Volland/lean-elpi), the ELPI dialect of λProlog implemented in Lean 4.
Higher-order rules, hypothetical reasoning (`=>`, `pi`), cut and negation as failure, over the
triples of the store.

```rust
use oxilite::store::Store;

let store = Store::new()?;
store.update(
    "PREFIX ex: <http://example.org/>
     INSERT DATA { ex:ada ex:parent ex:bob . ex:bob ex:parent ex:cy }",
)?;

let program = r#"
  pred anc o:term, o:term.
  anc X Y :- ox.triple X (iri "http://example.org/parent") Y.
  anc X Z :- ox.triple X (iri "http://example.org/parent") Y, anc Y Z.
"#;
let r = store.elpi_query(program, r#"anc (iri "http://example.org/ada") Who"#)?;
assert_eq!(r.solutions.len(), 2);   // bob and cy
# Result::<_, Box<dyn std::error::Error>>::Ok(())
```

## Building

The engine is the native `libleanelpi` (darwin and linux). Build it from lean-elpi
(`scripts/build-ffi.sh`), point `LEANELPI_DIST` at its `dist/` directory, and enable
`oxilite = { version = "0.9", features = ["elpi-native"] }`. With `elpi` alone the API builds
and every run fails with `ElpiError::Unsupported`. Host calls are synchronous, so programs run on
blocking stores, not on Cloudflare D1.

## Reading the store

| Builtin | Answers |
|---|---|
| `ox.triple S P O` | the triples of the scope, graphs merged, each once |
| `ox.quad S P O G` | the quads, `default-graph` for the default graph |

Each call is one pattern scan whose pattern is the call's ground arguments. RDF terms are typed
ELPI values: `(iri "…")`, `(str "…")`, `(num 42)` and `(dbl 1.5)` (native ELPI numbers),
`(lang "chat" "fr")`, `(lit "2026-11-03" "http://www.w3.org/2001/XMLSchema#date")`, `(bnode "b0")`.

## Three uses

- `Store::elpi_query(program, goal)`: any goal; bindings come back as RDF terms.
- `Store::elpi_check(program)`: everything `ox.violation Severity Name Message Focus` reports, with
  severity `error` or `warning`.
- `Store::elpi_materialize(program)`: the triples of `ox.conclude (triple S P O)`, stored as
  inferences of the producer `elpi`.

`Options` set the scope (`graphs`, `include_inferred`), the `producer`, and the bounds: `limit`
(solutions), `fuel` (resolution steps) and `max_rows` (rows the builtins may return).

From the command line (CLI feature `elpi`): `oxilite elpi -l data.sqlite -f rules.elpi --goal '…'`,
`--check` or `--materialize`.

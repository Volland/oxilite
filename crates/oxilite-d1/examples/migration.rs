//! Prints the oxilite schema as a D1 migration: `cargo run -p oxilite-d1 --example migration`.
fn main() {
    let graph_index = !std::env::args().any(|a| a == "--no-graph-index");
    print!("{}", oxilite_d1::migration_sql(graph_index));
}

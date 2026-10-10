fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/specs/permissions-matrix.md");
    let expected = porchlight_api::permissions::permissions_markdown();
    let count: usize = porchlight_api::permissions::endpoint_tables()
        .iter()
        .map(|table| porchlight_api::permissions::matrix_rows(table).len())
        .sum();
    let core_count = porchlight_core::access::permission_matrix().len();
    if std::env::args().any(|a| a == "--check") {
        if std::fs::read_to_string(&path)? != expected {
            return Err("Endpoint permissions matrix is stale; regenerate with the API endpoint-matrix example".into());
        }
        println!("Endpoint permissions matrix is current ({count} HTTP rows)");
    } else {
        std::fs::write(path, expected)?;
        println!("Generated permissions matrix ({core_count} core + {count} HTTP rows)");
    }
    Ok(())
}

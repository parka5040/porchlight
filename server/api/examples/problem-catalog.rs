//! Regenerate or check the RFC 9457 problem catalog from the closed API enum.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/problems.md");
    let expected = porchlight_api::error::Problem::markdown();
    if std::env::args().any(|arg| arg == "--check") {
        if std::fs::read_to_string(path)? != expected {
            return Err(
                "Problem catalog is stale; regenerate with the problem-catalog example".into(),
            );
        }
        println!("Problem catalog is current");
    } else {
        std::fs::write(path, expected)?;
        println!("Generated problem catalog");
    }
    Ok(())
}

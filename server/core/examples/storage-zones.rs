//! Installer preflight uses exactly the server's bundled storage-zone parser, not host tzdata.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut zones: Vec<_> = chrono_tz::TZ_VARIANTS
        .iter()
        .map(|zone| zone.name())
        .filter(|name| porchlight_core::time::storage_zone(name).is_ok())
        .collect();
    zones.sort_unstable();
    let expected = zones.join("\n") + "\n";
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/lib/storage-zones.txt");
    if std::env::args().any(|arg| arg == "--check") {
        if std::fs::read_to_string(path)? != expected {
            return Err(
                "Installer storage zones are stale; regenerate with the storage-zones example"
                    .into(),
            );
        }
        println!(
            "Installer storage zones are current ({} names)",
            zones.len()
        );
    } else {
        std::fs::write(path, expected)?;
    }
    Ok(())
}

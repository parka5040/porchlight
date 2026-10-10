//! Regenerate the checked-in core permission matrix, or check it with `--check`.
use porchlight_core::access::{permission_matrix, permissions_markdown};

#[path = "permissions-matrix/document_check.rs"]
mod document_check;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/specs/permissions-matrix.md");
    let document = permissions_markdown();
    let row_count = permission_matrix().len();
    match std::env::args().nth(1).as_deref() {
        None => {
            let suffix = std::fs::read_to_string(&path)
                .ok()
                .and_then(|actual| {
                    actual
                        .split_once("\n<!-- HTTP endpoint matrix -->\n")
                        .map(|(_, suffix)| format!("\n<!-- HTTP endpoint matrix -->\n{suffix}"))
                })
                .unwrap_or_default();
            std::fs::write(&path, format!("{document}{suffix}"))?;
            println!("Generated docs/specs/permissions-matrix.md ({row_count} rows)");
        }
        Some("--check") => {
            document_check::check_document(&path, &document)?;
            println!("Permissions matrix is current ({row_count} rows)");
        }
        Some(_) => return Err("usage: permissions-matrix [--check]".into()),
    }
    Ok(())
}

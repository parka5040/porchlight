use std::{error::Error, path::Path};

const REGENERATE: &str = "regenerate from the repository root: CARGO_NET_OFFLINE=true cargo run --manifest-path server/Cargo.toml -p porchlight-core --example permissions-matrix";

pub(super) fn check_document(path: &Path, expected: &str) -> Result<(), Box<dyn Error>> {
    let actual = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "could not read permissions matrix at {}: {error}",
            path.display()
        )
    })?;
    // The API owns the additive HTTP dimension; core checks only its own prefix.
    let core = actual
        .split("\n<!-- HTTP endpoint matrix -->\n")
        .next()
        .unwrap_or(&actual);
    check_contents(core, expected).map_err(Into::into)
}

pub(super) fn check_contents(actual: &str, expected: &str) -> Result<(), String> {
    // The API owns the independently generated endpoint suffix, checked by its own gate.
    let actual = actual
        .split("<!-- HTTP endpoint matrix -->")
        .next()
        .unwrap();
    if actual == expected {
        return Ok(());
    }
    // split rather than lines preserves trailing newlines; never dump either document.
    let mut actual_lines = actual.split('\n');
    let mut expected_lines = expected.split('\n');
    let mut line = 1;
    while actual_lines.next() == expected_lines.next() {
        line += 1;
    }
    Err(format!(
        "permissions matrix is stale (first differing line: {line}); {REGENERATE}"
    ))
}

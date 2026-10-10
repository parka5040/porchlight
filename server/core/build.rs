use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=data/breached-passwords.txt");
    let input = fs::read_to_string("data/breached-passwords.txt")
        .expect("could not read breached password list");
    let mut entries: Vec<String> = input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_lowercase)
        .filter(|entry| entry.chars().count() >= 12)
        .collect();
    entries.sort_unstable();
    entries.dedup();
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR is set"));
    fs::write(output.join("breached.txt"), entries.join("\n"))
        .expect("could not write compiled breached password list");
}

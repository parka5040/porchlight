//! Shared wire fixtures for verifier unit tests and the real CLI download path.
use std::io::Write;

fn encoder(complete: bool) -> flate2::write::GzEncoder<Vec<u8>> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in [
        ("data.json", b"{\"format_version\":2}".as_slice()),
        (
            "complete.json",
            b"{\"format_version\":2,\"complete\":true}".as_slice(),
        ),
    ] {
        if name == "complete.json" && !complete {
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes).unwrap();
    }
    tar.into_inner().unwrap()
}

pub fn archive(complete: bool) -> Vec<u8> {
    encoder(complete).finish().unwrap()
}

pub fn unfinished_deflate() -> Vec<u8> {
    let mut gzip = encoder(true);
    // All tar bytes (including completion/footer) have been supplied. A sync
    // flush exposes them without emitting deflate's final block or gzip trailer.
    gzip.flush().unwrap();
    gzip.get_ref().clone()
}

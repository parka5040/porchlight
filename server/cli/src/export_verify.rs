//! Bounded streaming household archive validation; no output-size limit or spool.
use std::io::{self, Read};

pub(crate) fn verify(reader: impl Read) -> io::Result<()> {
    let mut archive = tar::Archive::new(flate2::read::MultiGzDecoder::new(reader));
    let mut complete = false;
    let mut manifest = false;
    #[derive(serde::Deserialize)]
    struct Manifest {
        format_version: u32,
    }
    #[derive(serde::Deserialize)]
    struct Completion {
        format_version: u32,
        complete: bool,
    }
    for entry in archive.entries()? {
        let mut entry = entry?;
        if complete {
            return Err(io::Error::other("entry after completion"));
        }
        let path = entry.path()?.into_owned();
        if path == std::path::Path::new("complete.json") {
            let marker: Completion = serde_json::from_reader(&mut entry)?;
            complete = marker.format_version == 2 && marker.complete;
            if !complete {
                return Err(io::Error::other("invalid completion"));
            }
        } else {
            if path == std::path::Path::new("data.json") {
                if manifest
                    || serde_json::from_reader::<_, Manifest>(&mut entry)?.format_version != 2
                {
                    return Err(io::Error::other("invalid manifest"));
                }
                manifest = true;
            }
            io::copy(&mut entry, &mut io::sink())?;
        }
    }
    // Tar stops at its zero blocks, before gzip necessarily validates CRC/ISIZE.
    // Drain to transport EOF so a missing/corrupt trailer is never accepted.
    let mut decoder = archive.into_inner();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = decoder.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        if buffer[..n].iter().any(|b| *b != 0) {
            return Err(io::Error::other("data after tar end"));
        }
    }
    if !manifest || !complete {
        return Err(io::Error::other("incomplete household archive"));
    }
    Ok(())
}

struct ChannelReader {
    receive: tokio::sync::mpsc::Receiver<hyper::body::Bytes>,
    current: hyper::body::Bytes,
}
impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.current.is_empty() {
            match self.receive.blocking_recv() {
                Some(bytes) => self.current = bytes,
                None => return Ok(0),
            }
        }
        let n = out.len().min(self.current.len());
        out[..n].copy_from_slice(&self.current[..n]);
        self.current = self.current.slice(n..);
        Ok(n)
    }
}

pub(crate) struct Verification {
    send: tokio::sync::mpsc::Sender<hyper::body::Bytes>,
    worker: tokio::task::JoinHandle<io::Result<()>>,
}
impl Verification {
    pub(crate) fn start() -> Self {
        let (send, receive) = tokio::sync::mpsc::channel(2);
        let worker = tokio::task::spawn_blocking(move || {
            verify(ChannelReader {
                receive,
                current: hyper::body::Bytes::new(),
            })
        });
        Self { send, worker }
    }
    pub(crate) async fn chunk(&self, bytes: hyper::body::Bytes) -> crate::client::Result<()> {
        // Bound queued verification memory independently of the peer's frame size.
        for part in bytes.chunks(64 * 1024) {
            self.send
                .send(hyper::body::Bytes::copy_from_slice(part))
                .await
                .map_err(|_| crate::client::Error::Local("Incomplete household archive"))?;
        }
        Ok(())
    }
    pub(crate) async fn finish(self) -> crate::client::Result<()> {
        drop(self.send);
        self.worker
            .await
            .map_err(|_| crate::client::Error::Local("Could not verify household archive"))?
            .map_err(|_| crate::client::Error::Local("Incomplete household archive"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn archive(complete: bool) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::default(),
        ));
        for (name, data) in [
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
            header.set_mode(0o600);
            header.set_size(data.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, name, data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }
    #[test]
    fn completion_and_gzip_trailer_are_both_required() {
        assert!(verify(archive(true).as_slice()).is_ok());
        assert!(verify(archive(false).as_slice()).is_err());
        let mut truncated = archive(true);
        truncated.truncate(truncated.len() - 8);
        assert!(verify(truncated.as_slice()).is_err());
    }
}

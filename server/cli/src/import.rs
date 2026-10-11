//! One-off calendar import, using household-scoped username, display name or UUID targets.
use crate::client::{Client, Error, Result};
use clap::Subcommand;
use serde_json::{Value, json};
use std::{io::Read, path::PathBuf};
#[derive(Subcommand)]
pub enum Command {
    Ics {
        file: PathBuf,
        #[arg(long)]
        member: String,
        #[arg(long, default_value = "import")]
        name: String,
        /// Stable scope for reimport identity (defaults to import, matching HTTP).
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        visibility: Option<String>,
    },
}
const MAX_BYTES: u64 = 5 * 1024 * 1024;
fn read_content(reader: impl std::io::Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Local("Could not read calendar file"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::Local("Calendar file exceeds 5 MB"));
    }
    String::from_utf8(bytes).map_err(|_| Error::Local("Could not read UTF-8 calendar file"))
}
fn read_file(path: &std::path::Path) -> Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    // Linux O_NONBLOCK: opening a FIFO must not wait for a writer before fstat.
    // The CLI targets Linux; regular file reads ignore this flag.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(0o4000)
        .open(path)
        .map_err(|_| Error::Local("Could not read calendar file"))?;
    if !file
        .metadata()
        .map_err(|_| Error::Local("Could not inspect calendar file"))?
        .is_file()
    {
        return Err(Error::Local("Calendar must be a regular file"));
    }
    read_content(file)
}

pub async fn run(client: &Client, command: &Command) -> Result<Value> {
    match command {
        Command::Ics {
            file,
            member,
            name,
            scope,
            preview,
            visibility,
        } => {
            let file = file.clone();
            let content = tokio::task::spawn_blocking(move || read_file(&file))
                .await
                .map_err(|_| Error::Local("Could not read calendar file"))??;
            let member = client.calendar_member(member).await?;
            client.request("POST", "/v1/imports/ics", json!({"content":content,"member_id":member,"name":name,"scope":scope.as_deref().unwrap_or("import"),"preview":preview,"visibility":visibility})).await
        }
    }
}

#[cfg(test)]
mod audit_authz {
    use super::*;
    use std::io::Read;
    #[test]
    fn rejects_non_regular_files_and_invalid_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("calendar.ics");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(matches!(
            read_file(&fifo),
            Err(Error::Local("Calendar must be a regular file"))
        ));
        assert!(matches!(
            read_file(dir.path()),
            Err(Error::Local("Calendar must be a regular file"))
        ));
        assert!(read_content(&[0xff][..]).is_err());
        assert_eq!(
            read_content(std::io::repeat(b'a').take(MAX_BYTES))
                .unwrap()
                .len(),
            MAX_BYTES as usize
        );
    }
    #[test]
    fn read_caps_growth_after_metadata() {
        let mut reader = std::io::repeat(b'a').take(6 * 1024 * 1024);
        let result = read_content(&mut reader);
        assert!(
            result.is_err(),
            "growing calendar must be rejected by the read, not stat"
        );
        assert_eq!(
            reader.limit(),
            1024 * 1024 - 1,
            "read must stop at 5 MiB plus one byte"
        );
    }
}

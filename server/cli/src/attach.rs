use crate::client::{Client, Error, Result};
use serde_json::Value;
use std::path::Path;
use tokio::io::AsyncReadExt;
use uuid::Uuid;
pub async fn run(client: &Client, item: Uuid, file: &Path) -> Result<Value> {
    let f = tokio::fs::File::open(file)
        .await
        .map_err(|_| Error::Local("Could not open attachment"))?;
    if !f
        .metadata()
        .await
        .map_err(|_| Error::Local("Could not inspect attachment"))?
        .is_file()
    {
        return Err(Error::Local("Attachment must be a regular file"));
    }
    let mut bytes = Vec::new();
    f.take(25 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Error::Local("Could not read attachment"))?;
    if bytes.len() > 25 * 1024 * 1024 {
        return Err(Error::Local("Attachment exceeds 25 MiB"));
    }
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("attachment");
    client
        .upload(&format!("/v1/items/{item}/attachments"), name, &bytes)
        .await
}

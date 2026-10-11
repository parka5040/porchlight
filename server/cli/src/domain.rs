//! Shared paging, references, optimistic versions and safe human tables.
use crate::client::{Client, Error, Result, segment};
use serde_json::{Value, json};
use uuid::Uuid;
pub fn query(path: &str, args: &[(&str, Option<String>)]) -> String {
    let mut p = path.to_owned();
    for (k, v) in args {
        if let Some(v) = v {
            p.push(if p.contains('?') { '&' } else { '?' });
            p.push_str(k);
            p.push('=');
            p.push_str(&segment(v));
        }
    }
    p
}
pub async fn collection(c: &Client, path: &str) -> Result<Value> {
    let mut result = c.request("GET", path, Value::Null).await?;
    let mut items = result["items"]
        .as_array()
        .ok_or(Error::Local("Expected collection"))?
        .clone();
    let mut cursor = result["next_cursor"].as_str().map(str::to_owned);
    let mut seen = std::collections::BTreeSet::new();
    while let Some(next) = cursor {
        if !seen.insert(next.clone()) {
            return Err(Error::Local("Repeated page cursor"));
        }
        let page = c
            .request("GET", &query(path, &[("cursor", Some(next))]), Value::Null)
            .await?;
        items.extend(
            page["items"]
                .as_array()
                .ok_or(Error::Local("Expected collection"))?
                .iter()
                .cloned(),
        );
        cursor = page["next_cursor"].as_str().map(str::to_owned);
    }
    result["items"] = json!(items);
    result["next_cursor"] = Value::Null;
    Ok(result)
}
/// Find one member of a parent-bound collection without reading irrelevant later pages.
pub async fn collection_row(c: &Client, path: &str, id: Uuid) -> Result<Option<Value>> {
    find_row(id, |cursor| {
        let page = query(path, &[("cursor", cursor)]);
        async move { c.request("GET", &page, Value::Null).await }
    })
    .await
}
async fn find_row<F, Fut>(id: Uuid, mut fetch: F) -> Result<Option<Value>>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    let id = id.to_string();
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let page = fetch(cursor).await?;
        let items = page["items"]
            .as_array()
            .ok_or(Error::Local("Expected collection"))?;
        if let Some(row) = items.iter().find(|row| row["id"] == id) {
            return Ok(Some(row.clone()));
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        let Some(next) = cursor.as_ref() else {
            return Ok(None);
        };
        if !seen.insert(next.clone()) {
            return Err(Error::Local("Repeated page cursor"));
        }
    }
}

pub async fn member(c: &Client, name: &str) -> Result<Uuid> {
    if let Ok(id) = Uuid::parse_str(name) {
        return Ok(id);
    }
    let values = collection(c, "/v1/members").await?;
    let matches: Vec<_> = values["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| {
            m["username"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(name))
                || m["display_name"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
        })
        .collect();
    if matches.len() == 1 {
        return Uuid::parse_str(
            matches[0]["id"]
                .as_str()
                .ok_or(Error::Local("Missing member id"))?,
        )
        .map_err(|_| Error::Local("Invalid member id"));
    }
    if matches.len() > 1 {
        return Err(Error::Usage(
            "Member name must resolve uniquely; use a UUID",
        ));
    }
    let own = c.request("GET", "/v1/admin/whoami", Value::Null).await?;
    if own["username"]
        .as_str()
        .is_some_and(|s| s.eq_ignore_ascii_case(name))
    {
        return Uuid::parse_str(
            own["member_id"]
                .as_str()
                .ok_or(Error::Local("Missing member id"))?,
        )
        .map_err(|_| Error::Local("Invalid member id"));
    }
    if let Ok(users) = collection(c, "/v1/admin/users").await {
        let matches: Vec<_> = users["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| {
                m["deleted_at"].is_null()
                    && m["username"]
                        .as_str()
                        .is_some_and(|s| s.eq_ignore_ascii_case(name))
            })
            .collect();
        if matches.len() == 1 {
            return Uuid::parse_str(matches[0]["id"].as_str().unwrap())
                .map_err(|_| Error::Local("Invalid member id"));
        }
    }
    Err(Error::Usage(
        "Member name must resolve uniquely; use a UUID",
    ))
}
pub async fn members(c: &Client, names: &[String]) -> Result<Vec<Uuid>> {
    let mut out = vec![];
    for name in names {
        out.push(member(c, name).await?);
    }
    Ok(out)
}
pub async fn item_version(c: &Client, id: Uuid, version: Option<i64>) -> Result<i64> {
    if let Some(v) = version {
        return Ok(v);
    }
    let v = c
        .request("GET", &format!("/v1/items/{id}"), Value::Null)
        .await?;
    v["version"]
        .as_i64()
        .ok_or(Error::Local("Missing item version"))
}
pub async fn occurrence_version(c: &Client, id: Uuid, version: Option<i64>) -> Result<i64> {
    if let Some(v) = version {
        return Ok(v);
    }
    let v = c.request("GET", "/v1/snapshot", Value::Null).await?;
    if let Some(row) = v["occurrences"]
        .as_array()
        .and_then(|xs| xs.iter().find(|r| r["id"] == id.to_string()))
    {
        return row["version"]
            .as_i64()
            .ok_or(Error::Local("Missing occurrence version"));
    }
    Err(Error::Usage(
        "Occurrence is outside the snapshot window; supply --version",
    ))
}
pub fn table(value: &Value) {
    fn cell(v: &Value) -> String {
        let s = match v {
            Value::String(s) => s.clone(),
            Value::Null => "-".into(),
            _ => v.to_string(),
        };
        s.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    }
    if let Some(rows) = value["items"].as_array() {
        println!("ID\tTITLE / NAME\tSTATUS / ROLE\tVERSION");
        for r in rows {
            println!(
                "{}\t{}\t{}\t{}",
                cell(
                    r.get("id")
                        .or_else(|| r.get("item_id"))
                        .unwrap_or(&Value::Null)
                ),
                cell(
                    r.get("title")
                        .or_else(|| r.get("display_name"))
                        .unwrap_or(&Value::Null)
                ),
                cell(
                    r.get("status")
                        .or_else(|| r.get("role"))
                        .unwrap_or(&Value::Null)
                ),
                cell(&r["version"])
            );
        }
    } else if let Some(o) = value.as_object() {
        for (k, v) in o {
            println!("{}\t{}", cell(&Value::String(k.clone())), cell(v));
        }
    }
}

#[cfg(test)]
mod audit_bl_server {
    use super::*;
    #[tokio::test]
    async fn occurrence_lookup_stops_on_the_matching_page() {
        let id = Uuid::nil();
        let mut calls = 0;
        let row = find_row(id, |_| {
            calls += 1;
            std::future::ready(Ok(if calls == 1 {
                json!({"items":[{"id":id,"version":7}],"next_cursor":"later"})
            } else {
                json!({"items":[],"next_cursor":null})
            }))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(row["version"], 7);
        assert_eq!(
            calls, 1,
            "lookup must not drain pages after finding the occurrence"
        );
    }
}

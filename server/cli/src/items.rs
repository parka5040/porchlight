use crate::{
    client::{Client, Error, Result},
    domain,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use clap::{Args, Subcommand};
use serde_json::{Map, Value, json};
use uuid::Uuid;
#[derive(Subcommand)]
pub enum Command {
    Add {
        #[command(flatten)]
        fields: Fields,
    },
    List {
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        include_deleted: bool,
    },
    Show {
        item: Uuid,
    },
    Edit {
        item: Uuid,
        #[command(flatten)]
        fields: Fields,
        #[arg(long)]
        version: Option<i64>,
        #[arg(long, default_value = "all")]
        scope: String,
        #[arg(long)]
        occurrence: Option<Uuid>,
    },
    Rm {
        item: Uuid,
        #[arg(long)]
        version: Option<i64>,
    },
    Assign {
        item: Uuid,
        #[arg(value_delimiter = ',')]
        members: Vec<String>,
        #[arg(long)]
        version: Option<i64>,
    },
    Viewers {
        item: Uuid,
        #[arg(value_delimiter = ',')]
        members: Vec<String>,
        #[arg(long)]
        version: Option<i64>,
    },
    Reminders {
        item: Uuid,
        #[arg(value_delimiter = ',')]
        offsets: Vec<i32>,
        #[arg(long)]
        version: Option<i64>,
    },
    Take {
        item: Uuid,
        #[arg(long)]
        version: Option<i64>,
    },
}
#[derive(Args, Default)]
pub struct Fields {
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    notes: Option<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    owner: Option<String>,
    #[arg(long)]
    start: Option<String>,
    #[arg(long)]
    end: Option<String>,
    #[arg(long)]
    due: Option<String>,
    #[arg(long)]
    date: Option<String>,
    #[arg(long)]
    tz: Option<String>,
    #[arg(long)]
    floating: bool,
    #[arg(long)]
    all_day: bool,
    #[arg(long)]
    rrule: Option<String>,
    #[arg(long)]
    every: Option<String>,
    #[arg(long)]
    missed: Option<String>,
    #[arg(long)]
    subject: Option<String>,
    #[arg(long, value_delimiter = ',')]
    assign: Vec<String>,
    #[arg(long)]
    visibility: Option<String>,
    #[arg(long)]
    buffer_before: Option<i32>,
    #[arg(long)]
    buffer_after: Option<i32>,
    #[arg(long)]
    est: Option<i32>,
    #[arg(long)]
    priority: Option<i16>,
    #[arg(long)]
    address: Option<String>,
    /// Clear nullable fields by their wire names (e.g. --clear subject_member_id).
    #[arg(long, value_delimiter = ',')]
    clear: Vec<String>,
}
fn local(value: &str) -> Result<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M"))
        .map_err(|_| Error::Usage("Use an ISO local time or an RFC3339 timestamp"))
}
fn interval(value: &str) -> Result<Value> {
    let n = value.bytes().take_while(u8::is_ascii_digit).count();
    let amount = value[..n]
        .parse::<i32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or(Error::Usage(
            "Interval: positive number followed by m, h, d, w, mo or y",
        ))?;
    let unit = value[n..].trim();
    let mut months = 0;
    let mut days = 0;
    let mut micros = 0i64;
    match unit {
        "m" | "min" => micros = i64::from(amount) * 60_000_000,
        "h" => micros = i64::from(amount) * 3_600_000_000,
        "d" => days = amount,
        "w" => {
            days = amount
                .checked_mul(7)
                .ok_or(Error::Usage("Interval too large"))?
        }
        "mo" => months = amount,
        "y" => {
            months = amount
                .checked_mul(12)
                .ok_or(Error::Usage("Interval too large"))?
        }
        _ => {
            return Err(Error::Usage(
                "Interval: positive number followed by m, h, d, w, mo or y",
            ));
        }
    };
    Ok(json!({"months":months,"days":days,"microseconds":micros}))
}
impl Fields {
    async fn body(
        &self,
        c: &Client,
        existing: Option<&Value>,
        occurrence_timing: bool,
    ) -> Result<Value> {
        let mut b = Map::new();
        for (k, v) in [
            ("title", &self.title),
            ("notes", &self.notes),
            ("kind", &self.kind),
            ("visibility", &self.visibility),
            ("address", &self.address),
        ] {
            if let Some(v) = v {
                b.insert(k.into(), json!(v));
            }
        }
        for (k, v) in [
            ("buffer_before_min", self.buffer_before),
            ("buffer_after_min", self.buffer_after),
            ("est_minutes", self.est),
        ] {
            if let Some(v) = v {
                b.insert(k.into(), json!(v));
            }
        }
        if let Some(v) = self.priority {
            b.insert("priority".into(), json!(v));
        }
        for (k, v) in [
            ("owner_id", &self.owner),
            ("subject_member_id", &self.subject),
        ] {
            if let Some(v) = v {
                b.insert(k.into(), json!(domain::member(c, v).await?));
            }
        }
        if !self.assign.is_empty() {
            b.insert(
                "assignees".into(),
                json!(domain::members(c, &self.assign).await?),
            );
        }
        if let Some(v) = &self.date {
            v.parse::<chrono::NaiveDate>()
                .map_err(|_| Error::Usage("Date must be YYYY-MM-DD"))?;
            b.insert("due_date".into(), json!(v));
        }
        let mode = if self.floating {
            "floating"
        } else {
            existing
                .and_then(|v| v["time_mode"].as_str())
                .unwrap_or("fixed")
        };
        let zone = self
            .tz
            .as_deref()
            .or_else(|| existing.and_then(|v| v["tzid"].as_str()));
        if self.floating {
            b.insert("time_mode".into(), json!("floating"));
            b.insert("tzid".into(), Value::Null);
        } else if let Some(z) = &self.tz {
            porchlight_core::time::storage_zone(z)
                .map_err(|_| Error::Usage("Invalid time zone"))?;
            b.insert("tzid".into(), json!(z));
        }
        if self.all_day {
            b.insert("all_day".into(), json!(true));
        }
        let mut override_offsets = Vec::new();
        let mut override_locals = std::collections::BTreeMap::new();
        for (k, v) in [
            ("start", &self.start),
            ("end", &self.end),
            ("due", &self.due),
        ] {
            if let Some(v) = v {
                if occurrence_timing {
                    // Overrides persist nominal wall time even for fixed series.
                    // Do not resolve a local DST gap then round-trip it: that loses
                    // the requested nominal anchor. Offset inputs use the item zone.
                    let l = if mode == "fixed" {
                        if let Ok(instant) = DateTime::parse_from_rfc3339(v) {
                            let z = porchlight_core::time::parse_tz(zone.ok_or(Error::Usage(
                                "Fixed overrides require an item time zone",
                            ))?)
                            .ok_or(Error::Usage("Invalid time zone"))?;
                            let instant = instant.with_timezone(&Utc);
                            override_offsets.push((k, instant, z));
                            porchlight_core::time::to_local(instant, z)
                        } else {
                            local(v)?
                        }
                    } else {
                        local(v)?
                    };
                    override_locals.insert(k, l);
                    b.insert(format!("local_{k}"), json!(l));
                } else if mode == "floating" {
                    let l = local(v)?;
                    b.insert(format!("local_{k}"), json!(l));
                } else {
                    let utc = if let Ok(v) = DateTime::parse_from_rfc3339(v) {
                        v.with_timezone(&Utc)
                    } else {
                        let z = porchlight_core::time::parse_tz(
                            zone.ok_or(Error::Usage("Local fixed times require --tz"))?,
                        )
                        .ok_or(Error::Usage("Invalid time zone"))?;
                        porchlight_core::time::resolve_local(local(v)?, z)
                    };
                    let key = match k {
                        "start" => "starts_at",
                        "end" => "ends_at",
                        _ => "due_at",
                    };
                    b.insert(key.into(), json!(utc));
                }
            }
        }
        for (field, expected, zone) in override_offsets {
            let resolved = match (
                field,
                override_locals.get("start"),
                override_locals.get("end"),
            ) {
                ("start" | "end", Some(start), Some(end)) => {
                    let (start, end) = porchlight_core::time::resolve_span(*start, *end, zone)
                        .map_err(|_| {
                            Error::Usage("Override end must follow start in item-zone wall time")
                        })?;
                    if field == "start" { start } else { end }
                }
                _ => porchlight_core::time::resolve_local(override_locals[field], zone),
            };
            if resolved != expected {
                return Err(Error::Usage(
                    "Override offset cannot be preserved: item-zone folds use the earlier offset; use a representable local time",
                ));
            }
        }
        if existing.is_none()
            && self.start.is_some()
            && self.end.is_none()
            && self
                .kind
                .as_deref()
                .is_none_or(|kind| kind == "appointment")
        {
            return Err(Error::Usage("Appointments with --start require --end"));
        }
        if self.start.is_some() && self.kind.is_none() && existing.is_none() {
            b.insert("kind".into(), json!("appointment"));
        } else if self.due.is_some() && self.kind.is_none() && existing.is_none() {
            b.insert("kind".into(), json!("deadline"));
        }
        if let Some(v) = &self.rrule {
            if self.every.is_some() {
                return Err(Error::Usage("--rrule and --every are mutually exclusive"));
            }
            b.insert("rrule".into(), json!(v));
            b.insert("recurrence_mode".into(), json!("calendar"));
        }
        if let Some(v) = &self.every {
            b.insert("completion_interval".into(), interval(v)?);
            b.insert("recurrence_mode".into(), json!("completion"));
        }
        if let Some(v) = &self.missed {
            b.insert("missed_policy".into(), json!(v.replace('-', "_")));
        }
        for k in &self.clear {
            if ![
                "subject_member_id",
                "address",
                "source",
                "est_minutes",
                "tzid",
                "due_date",
                "due_at",
                "starts_at",
                "ends_at",
                "local_due",
                "local_start",
                "local_end",
                "rrule",
                "recurrence_mode",
                "completion_interval",
            ]
            .contains(&k.as_str())
            {
                return Err(Error::Usage("Unsupported nullable field for --clear"));
            }
            b.insert(k.clone(), Value::Null);
        }
        Ok(Value::Object(b))
    }
}
pub async fn run(c: &Client, cmd: &Command) -> Result<Value> {
    match cmd {
        Command::Add { fields } => {
            if fields.title.is_none() {
                return Err(Error::Usage("items add requires --title"));
            }
            c.request("POST", "/v1/items", fields.body(c, None, false).await?)
                .await
        }
        Command::List {
            kind,
            include_deleted,
        } => {
            domain::collection(
                c,
                &domain::query(
                    "/v1/items",
                    &[
                        ("kind", kind.clone()),
                        ("include_deleted", include_deleted.then(|| "true".into())),
                    ],
                ),
            )
            .await
        }
        Command::Show { item } => {
            c.request("GET", &format!("/v1/items/{item}"), Value::Null)
                .await
        }
        Command::Edit {
            item,
            fields,
            version,
            scope,
            occurrence,
        } => {
            let old = c
                .request("GET", &format!("/v1/items/{item}"), Value::Null)
                .await?;
            let mut body = fields.body(c, Some(&old), scope == "this").await?;
            if body.get("assignees").is_some() {
                return Err(Error::Usage(
                    "Use items assign for whole-set assignment replacement",
                ));
            }
            if scope == "this" {
                let oid = occurrence.ok_or(Error::Usage("--scope this requires --occurrence"))?;
                let mut patch = json!({});
                for (k, v) in body.as_object().unwrap() {
                    let wire = match k.as_str() {
                        "local_start" => "override_local_start",
                        "local_end" => "override_local_end",
                        "local_due" => "override_local_due",
                        "due_date" => "override_due_date",
                        _ => {
                            return Err(Error::Usage(
                                "This-occurrence edits allow local timing overrides only",
                            ));
                        }
                    };
                    patch[wire] = v.clone();
                }
                // Always verify membership, even when --version was supplied.
                // The item collection also includes preserved/out-of-window history;
                // a household snapshot is neither a parent check nor a complete list.
                let row = domain::collection_row(c, &format!("/v1/items/{item}/occurrences"), oid)
                    .await?
                    .ok_or(Error::Usage(
                        "Occurrence does not belong to the specified item",
                    ))?;
                let ver = version
                    .or_else(|| row["version"].as_i64())
                    .ok_or(Error::Local("Missing occurrence version"))?;
                c.request_version("PATCH", &format!("/v1/occurrences/{oid}"), patch, Some(ver))
                    .await
            } else {
                body["scope"] = json!(scope.replace('-', "_"));
                if let Some(v) = occurrence {
                    body["occurrence_id"] = json!(v);
                }
                let ver = version
                    .or_else(|| old["version"].as_i64())
                    .ok_or(Error::Local("Missing item version"))?;
                c.request_version("PATCH", &format!("/v1/items/{item}"), body, Some(ver))
                    .await
            }
        }
        Command::Rm { item, version } => write(c, *item, *version, "DELETE", "", json!({})).await,
        Command::Assign {
            item,
            members,
            version,
        } => {
            write(
                c,
                *item,
                *version,
                "PUT",
                "/assignees",
                json!({"assignees":domain::members(c,members).await?}),
            )
            .await
        }
        Command::Viewers {
            item,
            members,
            version,
        } => {
            write(
                c,
                *item,
                *version,
                "PUT",
                "/viewers",
                json!({"viewers":domain::members(c,members).await?}),
            )
            .await
        }
        Command::Reminders {
            item,
            offsets,
            version,
        } => {
            write(
                c,
                *item,
                *version,
                "PUT",
                "/reminders",
                json!({"reminder_offsets":offsets}),
            )
            .await
        }
        Command::Take { item, version } => {
            write(c, *item, *version, "POST", "/take", json!({})).await
        }
    }
}
async fn write(
    c: &Client,
    id: Uuid,
    version: Option<i64>,
    method: &str,
    suffix: &str,
    body: Value,
) -> Result<Value> {
    let version = domain::item_version(c, id, version).await?;
    c.request_version(
        method,
        &format!("/v1/items/{id}{suffix}"),
        body,
        Some(version),
    )
    .await
}

#[cfg(test)]
mod audit_bl_server {
    use super::*;

    #[tokio::test]
    async fn later_fold_override_offsets_are_not_silently_changed() {
        let client = Client::new("/srv/data/unused.sock".into(), String::new());
        let existing = json!({"time_mode":"fixed", "tzid":"America/New_York"});
        for (field, later, earlier) in [
            (
                "start",
                "2026-11-01T01:15:00-05:00",
                "2026-11-01T01:15:00-04:00",
            ),
            (
                "due",
                "2026-11-01T01:15:00-05:00",
                "2026-11-01T01:15:00-04:00",
            ),
        ] {
            for (text, accepted) in [(later, false), (earlier, true)] {
                let mut fields = Fields::default();
                if field == "start" {
                    fields.start = Some(text.into());
                    fields.end = Some("2026-11-01T02:15:00-05:00".into());
                } else {
                    fields.due = Some(text.into());
                }
                assert_eq!(
                    fields.body(&client, Some(&existing), true).await.is_ok(),
                    accepted,
                    "{field} fold identity"
                );
            }
        }
    }
}

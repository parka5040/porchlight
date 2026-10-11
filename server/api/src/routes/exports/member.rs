//! Complete JSON spool: one row/event at a time, one RR snapshot, no client-paced DB lease.
use super::*;
use futures_util::TryStreamExt;
use porchlight_core::ics::{EventOrigin, EventStatus, IcsTime, ImportedEvent, TimeMode};
use sqlx::{PgConnection, types::Json as SqlJson};
use std::io::{BufWriter, Seek, Write};

type Output = BufWriter<Spool>;
pub(super) struct Spool {
    pub file: std::fs::File,
    pub reservation: crate::blobs::DiskReservation,
    pub permit: crate::concurrency::Permit,
    state: AppState,
}
impl Write for Spool {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // Leave an additional reserve for administrative preparation. Retained
        // member spools remain charged alongside uploads until their reader drops.
        self.reservation
            .grow(
                &self.state.settings.data_dir,
                bytes.len() as u64,
                self.state.settings.disk_reserve_bytes.saturating_mul(2),
            )
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::StorageFull, "export capacity"))?;
        self.file.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
fn io<T>(value: std::io::Result<T>) -> Result<T, ApiError> {
    value.map_err(|e| {
        if e.kind() == std::io::ErrorKind::StorageFull {
            ApiError::new(Problem::StorageFull)
        } else {
            ApiError::internal("export_spool")
        }
    })
}
fn json(out: &mut Output, value: &impl serde::Serialize) -> Result<(), ApiError> {
    serde_json::to_writer(out, value).map_err(|e| {
        if e.io_error_kind() == Some(std::io::ErrorKind::StorageFull) {
            ApiError::new(Problem::StorageFull)
        } else {
            ApiError::internal("export_json")
        }
    })
}
// Escape a JSON string without buffering it. Non-ASCII bytes pass through unchanged,
// including when a UTF-8 sequence spans writes; the ICS writer only supplies valid UTF-8.
struct JsonString<'a>(&'a mut Output);
impl Write for JsonString<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut start = 0;
        for (i, b) in bytes.iter().copied().enumerate() {
            if b == b'"' || b == b'\\' || b < 32 {
                self.0.write_all(&bytes[start..i])?;
                match b {
                    b'"' => self.0.write_all(b"\\\"")?,
                    b'\\' => self.0.write_all(b"\\\\")?,
                    b'\r' => self.0.write_all(b"\\r")?,
                    b'\n' => self.0.write_all(b"\\n")?,
                    b'\t' => self.0.write_all(b"\\t")?,
                    _ => write!(self.0, "\\u{b:04x}")?,
                }
                start = i + 1;
            }
        }
        self.0.write_all(&bytes[start..])?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

async fn table(
    c: &mut PgConnection,
    caller: &Caller,
    out: &mut Output,
    sql: String,
) -> Result<(), ApiError> {
    io(out.write_all(b"["))?;
    let mut rows = sqlx::query_scalar::<_, SqlJson<Value>>(sqlx::AssertSqlSafe(sql))
        .bind(caller.household_id())
        .bind(caller.member_id())
        .bind(false)
        .fetch(c);
    let mut first = true;
    while let Some(SqlJson(row)) = rows.try_next().await? {
        if !first {
            io(out.write_all(b","))?;
        }
        first = false;
        json(out, &row)?;
    }
    io(out.write_all(b"]"))
}
async fn notifications(
    c: &mut PgConnection,
    caller: &Caller,
    out: &mut Output,
) -> Result<(), ApiError> {
    use crate::routes::notifications::{ROW, Row, detail};
    io(out.write_all(b"["))?;
    let mut before: Option<Uuid> = None;
    let mut first = true;
    loop {
        // Keyset reads allow detail's ACL rechecks on the SAME snapshot connection.
        let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{ROW} and ($3::uuid is null or id<$3) order by id desc limit 1"
        )))
        .bind(caller.household_id())
        .bind(caller.member_id())
        .bind(before)
        .fetch_optional(&mut *c)
        .await?;
        let Some(row) = row else { break };
        before = Some(row.id);
        if let Some(row) = detail(c, caller.member_id(), caller.household_id(), row).await? {
            if !first {
                io(out.write_all(b","))?;
            }
            first = false;
            json(out, &row)?;
        }
    }
    io(out.write_all(b"]"))
}
async fn sources(c: &mut PgConnection, caller: &Caller, out: &mut Output) -> Result<(), ApiError> {
    io(out.write_all(b"["))?;
    if caller.require_admin().is_ok() {
        let mut rows = sqlx::query_scalar::<_, SqlJson<Value>>("select jsonb_build_object('id',id,'name',name,'url',url,'enabled',enabled,'last_status',last_status,'last_fetch_at',last_fetch_at,'version',version) from porchlight.ics_sources where household_id=$1 and kind='feed' and deleted_at is null order by id")
            .bind(caller.household_id()).fetch(c);
        let mut first = true;
        while let Some(SqlJson(mut row)) = rows.try_next().await? {
            let url = row.as_object_mut().expect("source object").remove("url");
            let host = url
                .as_ref()
                .and_then(Value::as_str)
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.host_str().map(str::to_owned));
            row["host"] = serde_json::to_value(host).expect("host string");
            if !first {
                io(out.write_all(b","))?;
            }
            first = false;
            json(out, &row)?;
        }
    }
    io(out.write_all(b"]"))
}
async fn calendar(
    c: &mut PgConnection,
    caller: &Caller,
    out: &mut Output,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let mut writer = io(porchlight_core::ics::Writer::new(
        JsonString(out),
        "Porchlight appointments",
        now,
    ))?;
    let mut after: Option<Uuid> = None;
    let sql = format!(
        "select i.id from porchlight.items i where {} and i.kind='appointment' and ($4::uuid is null or i.id>$4) order by i.id limit 1",
        crate::routes::reads::VISIBLE
    );
    loop {
        let id: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(caller.household_id())
            .bind(caller.member_id())
            .bind(false)
            .bind(after)
            .fetch_optional(&mut *c)
            .await?;
        let Some(id) = id else { break };
        after = Some(id);
        let item = porchlight_db::derive::load_item(c, id)
            .await
            .map_err(|_| ApiError::internal("export_timing"))?;
        let spec = item
            .spec()
            .map_err(|_| ApiError::internal("export_timing"))?;
        // Move notes/title once per master; every occurrence reuses the same event.
        let mut event = ImportedEvent {
            uid: String::new(),
            recurrence_id: None,
            origin: EventOrigin::Single,
            summary: item.title,
            description: item.notes,
            location: String::new(),
            start: IcsTime::Fixed(now),
            end: IcsTime::Fixed(now),
            all_day: item.timing.all_day,
            time_mode: if item.timing.time_mode == porchlight_core::items::TimeMode::Floating {
                TimeMode::Floating
            } else {
                TimeMode::Fixed
            },
            tzid: item.timing.tzid,
            rrule: None,
            exdates: Vec::new(),
            sequence: 0,
            status: EventStatus::Confirmed,
        };
        let mut rows = sqlx::query_scalar::<_, SqlJson<porchlight_db::derive::OccurrenceRow>>(
            "select to_jsonb(o) from porchlight.item_occurrences o where item_id=$1 order by id",
        )
        .bind(id)
        .fetch(&mut *c);
        while let Some(SqlJson(row)) = rows.try_next().await? {
            let anchor = porchlight_db::derive::effective_anchor(&row, &spec)
                .map_err(|_| ApiError::internal("export_timing"))?;
            let floating = event.time_mode == TimeMode::Floating;
            let start = if event.all_day {
                anchor
                    .local_start
                    .map(|t| IcsTime::Date(t.date()))
                    .or_else(|| anchor.due_date.map(IcsTime::Date))
            } else if floating {
                anchor.local_start.map(IcsTime::Floating)
            } else {
                anchor.starts_at.map(IcsTime::Fixed)
            };
            let Some(start) = start else { continue };
            let end = if event.all_day {
                anchor
                    .local_end
                    .map(|t| IcsTime::Date(t.date()))
                    .or_else(|| match &start {
                        IcsTime::Date(d) => d.succ_opt().map(IcsTime::Date),
                        _ => None,
                    })
            } else if floating {
                anchor.local_end.map(IcsTime::Floating)
            } else {
                anchor.ends_at.map(IcsTime::Fixed)
            };
            let Some(end) = end else { continue };
            event.uid = format!("{}@porchlight", row.id);
            event.start = start;
            event.end = end;
            event.sequence = row.version as u32;
            event.status = if row.status == porchlight_core::items::Status::Skipped {
                EventStatus::Cancelled
            } else {
                EventStatus::Confirmed
            };
            io(writer.event(&event, &Default::default()))?;
        }
    }
    io(writer.finish())?;
    Ok(())
}

pub(super) async fn prepare(
    s: AppState,
    caller: Caller,
    target: Uuid,
    permit: crate::concurrency::Permit,
) -> Result<Spool, ApiError> {
    let reservation = s.disk_budget.reserve(
        &s.settings.data_dir,
        0,
        s.settings.disk_reserve_bytes.saturating_mul(2),
    )?;
    let file = io(tempfile::tempfile_in(&s.settings.data_dir))?;
    let mut out = BufWriter::with_capacity(
        64 * 1024,
        Spool {
            file,
            reservation,
            permit,
            state: s.clone(),
        },
    );
    let mut tx = porchlight_db::tx::begin(&s.pool).await?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *tx)
        .await?;
    let row: Option<(Option<Uuid>, String)> = sqlx::query_as("select user_id,role from porchlight.household_members where household_id=$1 and id=$2 and deleted_at is null")
        .bind(caller.household_id()).bind(target).fetch_optional(&mut *tx).await?;
    let (user, role) = row.ok_or_else(|| ApiError::new(Problem::NotFound))?;
    let member = Caller::Member {
        member_id: target,
        user_id: user.unwrap_or(Uuid::nil()),
        role,
        device_id: Uuid::nil(),
        session_id: Uuid::nil(),
        household_id: caller.household_id(),
        client_addr: None,
        request_id: Uuid::now_v7(),
    };
    let now = (s.item_clock)();
    io(out.write_all(b"{\"format_version\":1,\"exported_at\":"))?;
    json(&mut out, &now)?;
    io(out.write_all(b",\"member_id\":"))?;
    json(&mut out, &target)?;
    io(out.write_all(b",\"tables\":{"))?;
    let item_ids = format!(
        "select i.id from porchlight.items i where {}",
        crate::routes::reads::VISIBLE
    );
    let list_ids = "select l.id from porchlight.lists l where l.household_id=$1 and l.deleted_at is null and porchlight.pl_list_can_read(l.id,$2)";
    let mut tables = std::collections::BTreeMap::new();
    tables.insert("items", format!("t.id in ({item_ids})"));
    for name in [
        "item_occurrences",
        "item_assignees",
        "item_viewers",
        "item_reminders",
    ] {
        tables.insert(name, format!("t.item_id in ({item_ids})"));
    }
    for name in ["comments", "attachments"] {
        tables.insert(
            name,
            format!("t.deleted_at is null and t.item_id in ({item_ids})"),
        );
    }
    tables.insert("lists", format!("t.id in ({list_ids})"));
    tables.insert("list_viewers", format!("t.list_id in ({list_ids})"));
    tables.insert(
        "list_entries",
        format!("t.archived_at is null and t.list_id in ({list_ids})"),
    );
    tables.insert("list_entry_checks", format!("t.entry_id in (select e.id from porchlight.list_entries e where e.archived_at is null and e.list_id in ({list_ids}))"));
    for name in [
        "member_settings",
        "availability_windows",
        "availability_profiles",
    ] {
        tables.insert(
            name,
            "t.household_id=$1 and t.member_id=$2 and not $3".into(),
        );
    }
    for name in [
        "schedule_suggestions",
        "scheduled_reminders",
        "member_agenda",
    ] {
        tables.insert(
            name,
            format!("t.member_id=$2 and t.item_id in ({item_ids})"),
        );
    }
    tables.insert("member_daily_load", "t.member_id=$2".into());
    tables.insert("households", "t.id=$1".into());
    for name in [
        "notifications",
        "notification_prefs",
        "ics_sources",
        "household_members",
    ] {
        tables.insert(name, String::new());
    }
    for (index, (name, condition)) in tables.into_iter().enumerate() {
        if index != 0 {
            io(out.write_all(b","))?;
        }
        json(&mut out, &name)?;
        io(out.write_all(b":"))?;
        match name {
            "notifications" => notifications(&mut tx, &member, &mut out).await?,
            "notification_prefs" => json(&mut out, &crate::routes::notifications::prefs_row(&mut tx, &member, false).await?)?,
            "ics_sources" => sources(&mut tx, &member, &mut out).await?,
            "household_members" => table(&mut tx, &member, &mut out, format!("select {} from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.household_id=$1 and m.deleted_at is null and not $3 order by m.id", crate::routes::members::MEMBER)).await?,
            _ => table(&mut tx, &member, &mut out, format!("select to_jsonb(t)-'search_document'-'during' from porchlight.{name} t where $1::uuid is not null and $2::uuid is not null and not $3 and ({condition}) order by to_jsonb(t)::text")).await?,
        }
    }
    io(out.write_all(b"},\"appointments_ics\":\""))?;
    calendar(&mut tx, &member, &mut out, now).await?;
    io(out.write_all(b"\"}"))?;
    io(out.flush())?;
    tx.commit().await?;
    let mut spool = out
        .into_inner()
        .map_err(|_| ApiError::internal("export_spool"))?;
    io(spool.file.rewind())?;
    Ok(spool)
}

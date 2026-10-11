//! One planned lock acquisition for a composable transaction. No scope expansion after members.
use super::*;
use sqlx::types::Json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct ScopeRequest {
    pub items: Vec<Uuid>,
    /// Include the NEW agenda members before beginning (use `agenda_members` for edits).
    pub members: Vec<Uuid>,
    pub occurrences: Vec<Uuid>,
    /// Reopening a closed non-detached row may delete it if its rule no longer includes it.
    pub reopens: Vec<Uuid>,
    /// Closed occurrences whose keys will change during a split must be prelocked too.
    pub splits: Vec<(Uuid, u32)>,
    pub board_moves: Vec<BoardMove>,
    /// Member timezone writes enqueue all owner/subject/assignee dependents, even non-pending ones.
    pub member_zones: Vec<Uuid>,
    /// Member-zone scope: enqueue only; no projection/member-peer drain locks.
    pub enqueue_only: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardMove {
    pub occurrence: Uuid,
    pub before: Option<Uuid>,
    pub after: Option<Uuid>,
    pub status: core::Status,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Plan {
    items: Vec<Uuid>,
    members: Vec<Uuid>,
    occurrences: Vec<Uuid>,
    strong: Vec<Uuid>,
    boards: Vec<MovePlan>,
}
pub struct WriteScope<'a, 't> {
    pub(crate) tx: &'a mut Transaction<'t, Postgres>,
    pub(crate) household: Uuid,
    members: Vec<Uuid>,
    targets: BTreeSet<Uuid>,
    locked: BTreeSet<Uuid>,
    occurrences: BTreeSet<Uuid>,
    strong: BTreeSet<Uuid>,
    splits: Vec<(Uuid, u32)>,
    boards: Vec<(BoardMove, MovePlan)>,
    derive_order: Vec<(Uuid, Uuid)>,
    load_dates: Vec<(Uuid, chrono::NaiveDate)>,
    dirty: BTreeSet<Uuid>,
    board_only: BTreeSet<Uuid>,
    enqueue_only: bool,
    board_now: DateTime<Utc>,
}
fn scope_error(reason: &str) -> Error {
    core::ValidationError {
        field: "scope",
        reason: reason.into(),
    }
    .into()
}
/// Core agenda semantics, shared by planning NEW field/relationship sets and creation.
pub fn agenda_members(fields: &ItemWrite, sets: &Relationships) -> Vec<Uuid> {
    write_members(fields, sets)
}
async fn current_members(conn: &mut PgConnection, id: Uuid) -> Result<Vec<Uuid>> {
    let row = derive::load_item(conn, id).await?;
    let sets = relationships(conn, id).await?;
    Ok(write_members(&ItemWrite::from(&row), &sets))
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct MovePlan {
    // Only the planned neighbourhood and caller bounds, not the whole column.
    snapshot: Vec<(Uuid, String)>,
    ordered: Vec<Uuid>,
    keys: Vec<String>,
}
async fn board(
    conn: &mut PgConnection,
    household: Uuid,
    m: &BoardMove,
    now: DateTime<Utc>,
) -> Result<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as("select o.id,o.board_position from porchlight.pl_board_cards($1,$3) o where o.status=$2 order by o.board_position collate \"C\",o.id")
        .bind(household).bind(m.status.as_str()).bind(now).fetch_all(conn).await?)
}
/// R30: before means immediately after that domain card; after only validates
/// ordering when both are supplied. Visibility never narrows this domain.
fn neighbourhood(rows: &[(Uuid, String)], m: &BoardMove) -> Result<MovePlan> {
    let rows: Vec<_> = rows
        .iter()
        .filter(|r| r.0 != m.occurrence)
        .cloned()
        .collect();
    if m.before == Some(m.occurrence) || m.after == Some(m.occurrence) {
        return Err(ordering::OrderingError::InvalidBounds.into());
    }
    let before = m
        .before
        .map(|id| rows.iter().position(|r| r.0 == id).ok_or(Error::NotFound))
        .transpose()?;
    let after = m
        .after
        .map(|id| rows.iter().position(|r| r.0 == id).ok_or(Error::NotFound))
        .transpose()?;
    if before.is_some_and(|l| after.is_some_and(|h| l >= h)) {
        return Err(ordering::OrderingError::InvalidBounds.into());
    }
    let gap = before.map(|l| l + 1).or(after).unwrap_or(rows.len());
    let lo = gap.checked_sub(1).map(|i| rows[i].1.as_str());
    let hi = rows.get(gap).map(|r| r.1.as_str());
    let mut snapshot = Vec::new();
    for i in [
        before,
        after,
        gap.checked_sub(1),
        (gap < rows.len()).then_some(gap),
    ]
    .into_iter()
    .flatten()
    {
        if !snapshot.contains(&rows[i]) {
            snapshot.push(rows[i].clone());
        }
    }
    if let Ok(key) = ordering::key_between(lo, hi) {
        return Ok(MovePlan {
            snapshot,
            ordered: vec![m.occurrence],
            keys: vec![key],
        });
    }
    let keys: Vec<_> = rows.iter().map(|r| r.1.clone()).collect();
    let (start, end, keys) = ordering::insertion_run(&keys, gap)?;
    for row in &rows[start.saturating_sub(1)..(end + 1).min(rows.len())] {
        if !snapshot.contains(row) {
            snapshot.push(row.clone());
        }
    }
    let mut ordered: Vec<_> = rows[start..end].iter().map(|r| r.0).collect();
    ordered.insert(gap - start, m.occurrence);
    Ok(MovePlan {
        snapshot,
        ordered,
        keys,
    })
}
async fn plan(
    conn: &mut PgConnection,
    household: Uuid,
    request: &ScopeRequest,
    now: DateTime<Utc>,
) -> Result<Plan> {
    let mut targets: BTreeSet<Uuid> = request.items.iter().copied().collect();
    let specific: BTreeSet<Uuid> = request
        .occurrences
        .iter()
        .chain(&request.reopens)
        .copied()
        .collect();
    let mut board_rows = BTreeSet::new();
    let mut boards = Vec::new();
    for m in &request.board_moves {
        let rows = board(conn, household, m, now).await?;
        let movement = neighbourhood(&rows, m)?;
        board_rows.extend(movement.ordered.iter().copied());
        boards.push(movement);
    }
    let children: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "select id,item_id from porchlight.item_occurrences where household_id=$1 and id=any($2)",
    )
    .bind(household)
    .bind(specific.iter().copied().collect::<Vec<_>>())
    .fetch_all(&mut *conn)
    .await?;
    if children.len() != specific.len() {
        return Err(Error::NotFound);
    }
    targets.extend(children.into_iter().map(|(_, item)| item));
    let zone_targets: Vec<Uuid> = sqlx::query_scalar("select i.id from porchlight.items i where i.household_id=$1 and (i.owner_id=any($2) or i.subject_member_id=any($2) or exists(select 1 from porchlight.item_assignees a where a.item_id=i.id and a.member_id=any($2))) order by i.id").bind(household).bind(&request.member_zones).fetch_all(&mut *conn).await?;
    targets.extend(zone_targets);
    let mut members: BTreeSet<Uuid> = request
        .members
        .iter()
        .chain(&request.member_zones)
        .copied()
        .collect();
    for id in &targets {
        let row = derive::load_item(conn, *id).await?;
        if row.household_id != household {
            return Err(Error::NotFound);
        }
        if !request.enqueue_only {
            members.extend(current_members(conn, *id).await?);
        }
    }
    let stale: Vec<Uuid> = sqlx::query_scalar(
        "select distinct member_id from porchlight.member_agenda where item_id=any($1)",
    )
    .bind(targets.iter().copied().collect::<Vec<_>>())
    .fetch_all(&mut *conn)
    .await?;
    if !request.enqueue_only {
        members.extend(stale);
    }
    // Use the SAME core agenda rule for candidate peers, plus their stale derived membership.
    let candidates: Vec<Uuid> = sqlx::query_scalar("select i.id from porchlight.items i join porchlight.item_derivation d on d.item_id=i.id where i.household_id=$1 and i.deleted_at is null and d.applied_gen<d.requested_gen and d.attempts<5 order by i.id").bind(household).fetch_all(&mut *conn).await?;
    let stale_peers: Vec<Uuid> = sqlx::query_scalar("select distinct item_id from porchlight.member_agenda where household_id=$1 and member_id=any($2)").bind(household).bind(members.iter().copied().collect::<Vec<_>>()).fetch_all(&mut *conn).await?;
    let mut ids = targets;
    for id in candidates
        .into_iter()
        .filter(|_| !request.enqueue_only && !members.is_empty())
    {
        if stale_peers.contains(&id)
            || current_members(conn, id)
                .await?
                .iter()
                .any(|m| members.contains(m))
        {
            ids.insert(id);
        }
    }
    let mut reparent = BTreeSet::new();
    for (id, seq) in request.splits.iter().filter(|(_, seq)| *seq > 0) {
        if !request.items.contains(id) {
            return Err(scope_error("split head must be a target"));
        }
        let rows: Vec<Uuid> = sqlx::query_scalar(
            "select id from porchlight.item_occurrences where item_id=$1 and seq >= $2 order by id",
        )
        .bind(id)
        .bind(*seq as i64)
        .fetch_all(&mut *conn)
        .await?;
        reparent.extend(rows);
    }
    let mut occurrences: BTreeSet<Uuid> = sqlx::query_scalar::<_, Uuid>("select id from porchlight.item_occurrences where item_id=any($1) and status not in ('done','skipped') order by id").bind(ids.iter().copied().collect::<Vec<_>>()).fetch_all(&mut *conn).await?.into_iter().collect();
    if request.enqueue_only {
        occurrences.clear();
    }
    occurrences.extend(specific);
    occurrences.extend(reparent.iter().copied());
    let mut strong = reparent;
    let deletable: Vec<Uuid> = sqlx::query_scalar("select id from porchlight.item_occurrences where id=any($1) and not is_detached and (status not in ('done','skipped') or id=any($2)) order by id")
        .bind(occurrences.iter().copied().collect::<Vec<_>>()).bind(&request.reopens).fetch_all(&mut *conn).await?;
    strong.extend(deletable);
    occurrences.extend(board_rows);
    Ok(Plan {
        items: ids.into_iter().collect(),
        members: members.into_iter().collect(),
        occurrences: occurrences.into_iter().collect(),
        strong: strong.into_iter().collect(),
        boards,
    })
}
impl<'a, 't> WriteScope<'a, 't> {
    pub async fn begin(
        tx: &'a mut Transaction<'t, Postgres>,
        household: Uuid,
        request: ScopeRequest,
    ) -> Result<Self> {
        Self::begin_at(tx, household, request, Utc::now()).await
    }
    /// Begin with a caller-injected clock, frozen across planning and board rechecks.
    /// Use the same instant for domain mutations and `finish`; `begin` remains the
    /// production-clock convenience constructor.
    pub async fn begin_at(
        tx: &'a mut Transaction<'t, Postgres>,
        household: Uuid,
        request: ScopeRequest,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        Self::begin_checked(tx, household, request, now, || true).await
    }
    // A pure verifier permits deterministic acquisition-churn testing without sleeping
    // or changing production planner SQL. Called once after every acquired plan.
    async fn begin_checked(
        tx: &'a mut Transaction<'t, Postgres>,
        household: Uuid,
        request: ScopeRequest,
        now: DateTime<Utc>,
        mut verifier: impl FnMut() -> bool,
    ) -> Result<Self> {
        let marker: Option<String> =
            sqlx::query_scalar("select nullif(current_setting('porchlight.write_scope',true),'')")
                .fetch_one(&mut **tx)
                .await?;
        if marker.is_some() {
            return Err(Error::ScopeAlreadyUsed);
        }
        if !request.member_zones.is_empty() && !request.enqueue_only {
            return Err(scope_error("member-zone scopes require enqueue_only"));
        }
        if request.enqueue_only
            && (request.member_zones.is_empty()
                || !request.items.is_empty()
                || !request.occurrences.is_empty()
                || !request.reopens.is_empty()
                || !request.splits.is_empty()
                || !request.board_moves.is_empty()
                || !request.members.is_empty())
        {
            return Err(scope_error("enqueue_only is only for member-zone scopes"));
        }
        sqlx::query("select set_config('porchlight.write_scope','active',true)")
            .execute(&mut **tx)
            .await?;
        let mut initial = None;
        for _ in 0..3 {
            let mut attempt = tx.begin().await?;
            let p = plan(&mut attempt, household, &request, now).await?;
            sqlx::query(
                "select id from porchlight.items where id=any($1) order by id for no key update",
            )
            .bind(&p.items)
            .fetch_all(&mut *attempt)
            .await?;
            // One ordered statement, not O(rows) round trips. Board-only takes NKU;
            // derivation scopes use UPDATE if any row may be deleted/reparented.
            let sql = if p.strong.is_empty() {
                "select id from porchlight.item_occurrences where id=any($1) order by id for no key update"
            } else {
                "select id from porchlight.item_occurrences where id=any($1) order by id for update"
            };
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(&p.occurrences)
                .fetch_all(&mut *attempt)
                .await?;
            sqlx::query("select item_id from porchlight.item_derivation where item_id=any($1) order by item_id for update").bind(&p.items).fetch_all(&mut *attempt).await?;
            // Keep ordinary/co-member locks FK KEY SHARE compatible. Only the zone
            // targets need UPDATE, as the final lock statement to block new references.
            sqlx::query("select id from porchlight.household_members where household_id=$1 and id=any($2) order by id for no key update")
                .bind(household).bind(&p.members).fetch_all(&mut *attempt).await?;
            if !request.member_zones.is_empty() {
                sqlx::query("select id from porchlight.household_members where household_id=$1 and id=any($2) and deleted_at is null order by id for update")
                    .bind(household).bind(&request.member_zones).fetch_all(&mut *attempt).await?;
            }
            let rechecked = plan(&mut attempt, household, &request, now).await;
            if !matches!(rechecked,Ok(ref current) if *current==p) || !verifier() {
                attempt.rollback().await?;
                continue;
            }
            attempt.commit().await?;
            initial = Some(p);
            break;
        }
        let initial = initial.ok_or(Error::RetryableScope)?;
        let mut targets: BTreeSet<Uuid> = request.items.iter().copied().collect();
        let parents: Vec<Uuid> = sqlx::query_scalar(
            "select distinct item_id from porchlight.item_occurrences where id=any($1)",
        )
        .bind(
            request
                .occurrences
                .iter()
                .chain(&request.reopens)
                .copied()
                .collect::<Vec<_>>(),
        )
        .fetch_all(&mut **tx)
        .await?;
        targets.extend(parents);
        Ok(Self {
            tx,
            household,
            members: initial.members,
            targets,
            locked: initial.items.into_iter().collect(),
            occurrences: initial.occurrences.into_iter().collect(),
            strong: initial.strong.into_iter().collect(),
            splits: request.splits,
            boards: request
                .board_moves
                .into_iter()
                .zip(initial.boards)
                .collect(),
            derive_order: Vec::new(),
            load_dates: Vec::new(),
            dirty: BTreeSet::new(),
            board_only: BTreeSet::new(),
            enqueue_only: request.enqueue_only,
            board_now: now,
        })
    }
    /// S8a: materialize a declared target before applying imported exception anchors.
    /// No scope enlargement: existing closed rows still require explicit declarations.
    /// Newly generated rows belong to this transaction and need no acquisition locks.
    pub async fn materialize_calendar(
        &mut self,
        id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<Vec<derive::OccurrenceRow>> {
        self.item(id).await?;
        if self.enqueue_only {
            return Err(scope_error(
                "calendar materialization in enqueue-only scope",
            ));
        }
        let old: BTreeSet<Uuid> = derive::occurrences(self.tx, id)
            .await?
            .into_iter()
            .map(|o| o.id)
            .collect();
        derive::derive_item_prelocked(self.tx, id, now).await?;
        let rows = derive::occurrences(self.tx, id).await?;
        for row in &rows {
            if !old.contains(&row.id) {
                self.occurrences.insert(row.id);
            }
        }
        self.dirty.insert(id);
        Ok(rows)
    }
    /// Persist a source exception outside the normal horizon. Existing instances must already
    /// be in the acquisition plan; only genuinely new rows are admitted after member locks.
    pub async fn ensure_calendar_anchor(
        &mut self,
        item: Uuid,
        anchor: &core::OccurrenceAnchor,
    ) -> Result<derive::OccurrenceRow> {
        self.item(item).await?;
        let existing: Option<Uuid> = sqlx::query_scalar(
            "select id from porchlight.item_occurrences where item_id=$1 and seq=$2",
        )
        .bind(item)
        .bind(anchor.seq as i32)
        .fetch_optional(&mut **self.tx)
        .await?;
        let id = if let Some(id) = existing {
            if !self.occurrences.contains(&id) {
                return Err(Error::RetryableScope);
            }
            id
        } else {
            let id:Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,local_start,local_end,local_due,due_date,starts_at,ends_at,due_at) values($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) returning id")
                .bind(self.household).bind(item).bind(anchor.seq as i32).bind(anchor.local_start).bind(anchor.local_end).bind(anchor.local_due).bind(anchor.due_date).bind(anchor.starts_at).bind(anchor.ends_at).bind(anchor.due_at).fetch_one(&mut **self.tx).await?;
            self.occurrences.insert(id);
            id
        };
        self.dirty.insert(item);
        self.occurrence(id).await
    }
    /// Reset a source-managed occurrence's timing override without touching human completion.
    pub async fn clear_calendar_override(&mut self, id: Uuid, version: i32) -> Result<()> {
        let (item, _) = self.checked_occurrence(id, version).await?;
        sqlx::query("update porchlight.item_occurrences set override_local_start=null,override_local_end=null,override_local_due=null,override_due_date=null where id=$1 and (override_local_start is not null or override_local_end is not null or override_local_due is not null or override_due_date is not null)").bind(id).execute(&mut **self.tx).await?;
        self.dirty.insert(item.id);
        Ok(())
    }
    /// S8a feed identity is set only on retained scope targets (no late parent locks).
    pub async fn restore_calendar(&mut self, id: Uuid) -> Result<()> {
        self.assert_target(id)?;
        sqlx::query(
            "update porchlight.items set deleted_at=null where id=$1 and deleted_at is not null",
        )
        .bind(id)
        .execute(&mut **self.tx)
        .await?;
        self.dirty.insert(id);
        Ok(())
    }
    pub async fn mark_feed(&mut self, id: Uuid, source: Uuid) -> Result<()> {
        self.assert_target(id)?;
        sqlx::query("update porchlight.items set ics_source_id=$2 where id=$1 and ics_source_id is distinct from $2")
            .bind(id).bind(source).execute(&mut **self.tx).await?;
        Ok(())
    }
    fn assert_target(&self, id: Uuid) -> Result<()> {
        if !self.targets.contains(&id) {
            return Err(scope_error("item not declared as a target"));
        }
        Ok(())
    }
    pub async fn item(&mut self, id: Uuid) -> Result<ItemRow> {
        self.assert_target(id)?;
        derive::load_item(self.tx, id).await
    }
    async fn checked(&mut self, id: Uuid, version: i32) -> Result<ItemRow> {
        let row = self.item(id).await?;
        if row.version != version {
            return Err(Error::VersionConflict);
        }
        Ok(row)
    }
    pub async fn occurrence(&mut self, id: Uuid) -> Result<derive::OccurrenceRow> {
        if !self.occurrences.contains(&id) {
            return Err(scope_error("occurrence not prelocked"));
        }
        let Json(value) = sqlx::query_scalar::<_, Json<derive::OccurrenceRow>>(
            "select to_jsonb(o) from porchlight.item_occurrences o where id=$1",
        )
        .bind(id)
        .fetch_one(&mut **self.tx)
        .await?;
        Ok(value)
    }
    /// Recheck projection eligibility from authoritative anchors under the held
    /// item/occurrence/member locks, rather than using the aggregate board expiry.
    pub async fn recipient_disposition(
        &mut self,
        occurrence: Uuid,
        member: Uuid,
        now: DateTime<Utc>,
    ) -> Result<core::Disposition> {
        if !self.members.contains(&member) {
            return Err(scope_error("recipient member not prelocked"));
        }
        let occurrence = self.occurrence(occurrence).await?;
        let spec = self.item(occurrence.item_id).await?.spec()?;
        let zone: String = sqlx::query_scalar("select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=$1")
            .bind(member).fetch_one(&mut **self.tx).await?;
        let rows = derive::occurrences(self.tx, occurrence.item_id).await?;
        let index = rows
            .iter()
            .position(|o| o.id == occurrence.id)
            .ok_or(Error::NotFound)?;
        let effective: Vec<_> = rows
            .iter()
            .map(|o| {
                Ok(core::OpenOccurrence {
                    anchor: derive::effective_anchor(o, &spec)?,
                    status: o.status,
                    disposition: o.disposition,
                    closed_at: o.closed_at,
                    generated_anchor: o.is_detached.then(|| o.anchor.clone()),
                })
            })
            .collect::<Result<_>>()?;
        Ok(core::dispositions(
            &effective,
            now,
            spec.input().missed_policy,
            spec.zone().unwrap_or(derive::parse_zone(&zone)?),
        )?[index])
    }
    async fn checked_occurrence(
        &mut self,
        id: Uuid,
        version: i32,
    ) -> Result<(ItemRow, derive::OccurrenceRow)> {
        let o = self.occurrence(id).await?;
        if o.version != version {
            return Err(Error::VersionConflict);
        }
        Ok((self.item(o.item_id).await?, o))
    }
    async fn ensure_members(&mut self, fields: &ItemWrite, sets: &Relationships) -> Result<()> {
        if write_members(fields, sets)
            .iter()
            .any(|id| !self.members.contains(id))
        {
            return Err(scope_error("new agenda member omitted from ScopeRequest"));
        }
        Ok(())
    }
    pub async fn create(&mut self, fields: &ItemWrite, sets: &Relationships) -> Result<Uuid> {
        self.create_calendar(fields, sets, None).await
    }
    /// Source identity belongs in the initial row: no second trigger-firing write.
    pub async fn create_calendar(
        &mut self,
        fields: &ItemWrite,
        sets: &Relationships,
        source: Option<Uuid>,
    ) -> Result<Uuid> {
        self.ensure_members(fields, sets).await?;
        let spec = core::SeriesSpec::new(fields.timing.clone())?;
        let id = insert_calendar(
            self.tx,
            self.household,
            fields,
            &spec,
            (None, None, None),
            source,
        )
        .await?;
        // New rows have no children. Empty sets require no SQL at all.
        create_relationships(self.tx, self.household, id, sets).await?;
        self.targets.insert(id);
        self.locked.insert(id);
        self.dirty.insert(id);
        Ok(id)
    }
    pub async fn update(&mut self, id: Uuid, version: i32, fields: &ItemWrite) -> Result<()> {
        let row = self.checked(id, version).await?;
        let sets = relationships(self.tx, id).await?;
        self.ensure_members(fields, &sets).await?;
        let cursor = if changes_timing(&row.timing, &fields.timing) {
            None
        } else {
            row.nominal_local_anchor
                .map(|nominal_local_anchor| core::CalendarCursor {
                    nominal_local_anchor,
                    wall_duration: row
                        .nominal_wall_duration
                        .map(chrono::Duration::microseconds),
                })
        };
        let spec = core::SeriesSpec::new_with_calendar_cursor(fields.timing.clone(), cursor)?;
        persist(self.tx, id, fields, &spec).await?;
        self.dirty.insert(id);
        Ok(())
    }
    /// S5 revision restoration: use the same retained locks, validation, revision
    /// triggers and derivation as ordinary writes. Snapshot metadata is trusted
    /// only after the API selects a revision of this exact household/item.
    pub async fn restore_snapshot(
        &mut self,
        id: Uuid,
        version: i32,
        fields: &ItemWrite,
        sets: &Relationships,
        snapshot: &serde_json::Value,
    ) -> Result<()> {
        let current = self.checked(id, version).await?;
        // A single-item revision is not a lineage-wide split undo. Replaying old
        // timing on a head can rematerialize keys now owned by its surviving tails.
        // The retained parent lock serializes this check with new splits; include
        // soft-deleted tails because their authoritative occurrences still exist.
        let has_tail: bool = sqlx::query_scalar(
            "select exists(select 1 from porchlight.items where household_id=$2 and split_from_item_id=$1)",
        )
        .bind(id)
        .bind(self.household)
        .fetch_one(&mut **self.tx)
        .await?;
        let nominal_anchor: Option<chrono::NaiveDateTime> =
            serde_json::from_value(snapshot["nominal_local_anchor"].clone())
                .map_err(|_| scope_error("invalid revision nominal anchor"))?;
        let nominal_duration: Option<i64> =
            serde_json::from_value(snapshot["nominal_wall_duration"].clone())
                .map_err(|_| scope_error("invalid revision nominal duration"))?;
        if has_tail
            && (changes_timing(&current.timing, &fields.timing)
                || nominal_anchor != current.nominal_local_anchor
                || nominal_duration != current.nominal_wall_duration)
        {
            return Err(scope_error(
                "cannot restore series timing while split descendants survive",
            ));
        }
        self.ensure_members(fields, sets).await?;
        let spec = core::SeriesSpec::new(fields.timing.clone())?;
        persist(self.tx, id, fields, &spec).await?;
        restore_relationships(self.tx, self.household, id, sets).await?;
        sqlx::query("update porchlight.items i set (deleted_at,series_id,split_from_item_id,split_from_seq,nominal_local_anchor,nominal_wall_duration)=(r.deleted_at,r.series_id,r.split_from_item_id,r.split_from_seq,r.nominal_local_anchor,r.nominal_wall_duration) from jsonb_populate_record(null::porchlight.items,$2) r where i.id=$1")
            .bind(id).bind(snapshot).execute(&mut **self.tx).await?;
        self.dirty.insert(id);
        Ok(())
    }
    pub async fn soft_delete(&mut self, id: Uuid, version: i32, now: DateTime<Utc>) -> Result<()> {
        self.checked(id, version).await?;
        sqlx::query("update porchlight.items set deleted_at=$2 where id=$1")
            .bind(id)
            .bind(now)
            .execute(&mut **self.tx)
            .await?;
        self.dirty.insert(id);
        Ok(())
    }
    pub async fn replace_relationships(
        &mut self,
        id: Uuid,
        version: i32,
        sets: &Relationships,
    ) -> Result<()> {
        let row = self.checked(id, version).await?;
        self.ensure_members(&ItemWrite::from(&row), sets).await?;
        restore_relationships(self.tx, self.household, id, sets).await?;
        self.dirty.insert(id);
        Ok(())
    }
    pub async fn set_status(
        &mut self,
        occ: Uuid,
        version: i32,
        status: core::Status,
        completed_by: Option<Uuid>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let (row, old) = self.checked_occurrence(occ, version).await?;
        if !old.status.is_open()
            && status.is_open()
            && !old.is_detached
            && !self.strong.contains(&occ)
        {
            return Err(scope_error(
                "declare closed reopen in ScopeRequest.reopens before member locks",
            ));
        }
        sqlx::query("update porchlight.item_occurrences set status=$2,completed_by=$3,completed_at=$4,skipped_at=$5 where id=$1").bind(occ).bind(status.as_str()).bind(if status==core::Status::Done {completed_by} else {None}).bind(if status==core::Status::Done {Some(now)} else {None}).bind(if status==core::Status::Skipped {Some(now)} else {None}).execute(&mut **self.tx).await?;
        self.dirty.insert(row.id);
        Ok(())
    }
    pub async fn complete(
        &mut self,
        occ: Uuid,
        version: i32,
        member: Uuid,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.set_status(occ, version, core::Status::Done, Some(member), now)
            .await
    }
    pub async fn reopen(&mut self, occ: Uuid, version: i32) -> Result<()> {
        self.set_status(occ, version, core::Status::Todo, None, self.board_now)
            .await
    }
    pub async fn override_occurrence(
        &mut self,
        occ: Uuid,
        version: i32,
        replacement: &core::OccurrenceAnchor,
    ) -> Result<()> {
        let (row, o) = self.checked_occurrence(occ, version).await?;
        let v = core::persisted_occurrence_override(&row.spec()?, o.anchor.seq, replacement)?;
        sqlx::query("update porchlight.item_occurrences set override_local_start=$2,override_local_end=$3,override_local_due=$4,override_due_date=$5 where id=$1").bind(occ).bind(v.override_local_start).bind(v.override_local_end).bind(v.override_local_due).bind(v.override_due_date).execute(&mut **self.tx).await?;
        self.dirty.insert(row.id);
        Ok(())
    }
    pub async fn split(
        &mut self,
        id: Uuid,
        version: i32,
        at_seq: u32,
    ) -> Result<(Option<Uuid>, Uuid)> {
        if at_seq > 0 && !self.splits.contains(&(id, at_seq)) {
            return Err(scope_error("split boundary not predeclared"));
        }
        let row = self.checked(id, version).await?;
        let (head, tail) = core::this_and_future(&row.spec()?, at_seq)?;
        let Some(head) = head else {
            self.dirty.insert(id);
            return Ok((None, id));
        };
        let mut fields = ItemWrite::from(&row);
        fields.timing = head.input().clone();
        persist(self.tx, id, &fields, &head).await?;
        fields.timing = tail.input().clone();
        let tail_id = insert(
            self.tx,
            self.household,
            &fields,
            &tail,
            (
                Some(row.series_id.unwrap_or(id)),
                Some(id),
                Some(at_seq as i32),
            ),
        )
        .await?;
        let sets = relationships(self.tx, id).await?;
        // Clone explicit offsets with their delivery identity before set-difference
        // replacement; defaults inherit the head's reminder origin at item insertion.
        sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes,delivery_id) select household_id,$2,offset_minutes,coalesce(delivery_id,id) from porchlight.item_reminders where item_id=$1")
            .bind(id).bind(tail_id).execute(&mut **self.tx).await?;
        restore_relationships(self.tx, self.household, tail_id, &sets).await?;
        // Preserve occurrence identity and ALL authoritative state. Keys change, so these rows
        // were locked FOR UPDATE before member acquisition. Generated rows are rebuilt at finish.
        let old_dates: Vec<(Uuid, Option<chrono::NaiveDate>)> = sqlx::query_as("delete from porchlight.member_agenda a where occurrence_id in (select id from porchlight.item_occurrences where item_id=$1 and seq >= $2) returning member_id,coalesce((coalesce(a.starts_at,a.due_at) at time zone (select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=a.member_id))::date,a.due_date)")
            .bind(id).bind(at_seq as i32).fetch_all(&mut **self.tx).await?;
        // Tail edits can remove estimates, members or dates. Recompute these old
        // dates after BOTH sides converge, even if no new load date replaces them.
        self.load_dates
            .extend(old_dates.into_iter().filter_map(|(m, d)| d.map(|d| (m, d))));
        // A due reminder is a delivery obligation. Reparent without changing its
        // dedupe identity: a concurrent producer may still commit the head snapshot.
        // Tail derivation validates offsets/recipients/fire times and retires emitted
        // rows against the SAME witness, even after a backward clock correction.
        sqlx::query("update porchlight.scheduled_reminders r set item_id=$2 where r.item_id=$1 and r.occurrence_id in (select id from porchlight.item_occurrences where item_id=$1 and seq >= $3)")
            .bind(id).bind(tail_id).bind(at_seq as i32).execute(&mut **self.tx).await?;
        self.derive_order.push((id, tail_id));
        sqlx::query("update porchlight.item_occurrences set item_id=$2,seq=seq-$3 where item_id=$1 and seq >= $3").bind(id).bind(tail_id).bind(at_seq as i32).execute(&mut **self.tx).await?;
        self.targets.insert(tail_id);
        self.locked.insert(tail_id);
        self.dirty.insert(id);
        self.dirty.insert(tail_id);
        Ok((Some(id), tail_id))
    }
    pub async fn move_on_board(
        &mut self,
        occ: Uuid,
        version: i32,
        before: Option<Uuid>,
        after: Option<Uuid>,
    ) -> Result<()> {
        let o = self.occurrence(occ).await?;
        if o.version != version {
            return Err(Error::VersionConflict);
        }
        let m = BoardMove {
            occurrence: occ,
            before,
            after,
            status: o.status,
        };
        if !self.boards.iter().any(|(planned, _)| *planned == m) {
            return Err(scope_error("board move not predeclared"));
        }
        let rows = board(self.tx, self.household, &m, self.board_now).await?;
        if !rows.iter().any(|r| r.0 == occ) {
            return Err(Error::NotFound);
        }
        let movement = neighbourhood(&rows, &m).map_err(|_| Error::RetryableScope)?;
        if movement
            .ordered
            .iter()
            .any(|id| !self.occurrences.contains(id))
        {
            return Err(Error::RetryableScope);
        }
        let positions: BTreeMap<_, _> = rows.iter().map(|(id, key)| (*id, key)).collect();
        if movement
            .ordered
            .iter()
            .zip(&movement.keys)
            .all(|(id, key)| positions.get(id) == Some(&key))
        {
            self.board_only.insert(o.item_id);
            return Ok(());
        }
        // Trigger bookkeeping distinguishes an explicit move from automatic respread.
        // Restore the previous transaction-local value for later raw/composed writes.
        let previous: Option<String> =
            sqlx::query_scalar("select current_setting('porchlight.board_mover',true)")
                .fetch_one(&mut **self.tx)
                .await?;
        sqlx::query("select set_config('porchlight.board_mover',$1,true)")
            .bind(occ.to_string())
            .execute(&mut **self.tx)
            .await?;
        for (id, key) in movement.ordered.into_iter().zip(movement.keys) {
            self.position(id, &key, id == occ).await?;
        }
        sqlx::query("select set_config('porchlight.board_mover',$1,true)")
            .bind(previous.unwrap_or_default())
            .execute(&mut **self.tx)
            .await?;
        self.board_only.insert(o.item_id);
        Ok(())
    }
    async fn position(&mut self, id: Uuid, key: &str, mover: bool) -> Result<()> {
        if !self.occurrences.contains(&id) {
            return Err(scope_error("board neighbourhood not locked"));
        }
        // A real respread may preserve the mover's key while changing its history marker.
        // Already-marked rows with unchanged keys need no write or version bump.
        sqlx::query("update porchlight.item_occurrences set board_position=$2 where id=$1 and (board_position is distinct from $2 or ($3 and not board_moved))").bind(id).bind(key).bind(mover).execute(&mut **self.tx).await?;
        Ok(())
    }
    /// Derive each dirty target once; pending peers each get an isolated savepoint.
    /// Failed peers retain retry/error state; their stale agenda cannot enter the result.
    pub async fn finish(self, now: DateTime<Utc>) -> Result<Vec<WriteOutcome>> {
        if self.enqueue_only {
            sqlx::query("select set_config('porchlight.write_scope','finished',true)")
                .execute(&mut **self.tx)
                .await?;
            return Ok(Vec::new());
        }
        let mut poisoned: BTreeSet<Uuid> = sqlx::query_scalar("select d.item_id from porchlight.item_derivation d join porchlight.items i on i.id=d.item_id where i.household_id=$1 and d.applied_gen<d.requested_gen and d.attempts>=5").bind(self.household).fetch_all(&mut **self.tx).await?.into_iter().collect();
        let mut outcomes = BTreeMap::new();
        let mut load_dates = self.load_dates;
        for id in &self.locked {
            if self.dirty.contains(id) {
                continue;
            }
            let pending: bool = sqlx::query_scalar(
                "select applied_gen<requested_gen and attempts<5 from porchlight.item_derivation where item_id=$1",
            )
            .bind(id)
            .fetch_one(&mut **self.tx)
            .await?;
            if !pending {
                continue;
            }
            let mut sp = self.tx.begin().await?;
            match derive::derive_item_prelocked(&mut sp, *id, now).await {
                Ok(outcome) => {
                    load_dates.extend(outcome.load_dates);
                    sp.commit().await?;
                }
                Err(error) => {
                    tracing::warn!(item=%id, category=derive::error_category(&error), "peer derivation failed; retaining pending state");
                    sp.rollback().await?;
                    poisoned.insert(*id);
                }
            }
        }
        let mut dirty = self.dirty;
        while !dirty.is_empty() {
            let id = *dirty
                .iter()
                .find(|id| {
                    !self
                        .derive_order
                        .iter()
                        .any(|(head, tail)| tail == *id && dirty.contains(head))
                })
                .ok_or_else(|| scope_error("cyclic split dependency"))?;
            dirty.remove(&id);
            let mut outcome = derive::derive_item_prelocked(self.tx, id, now).await?;
            load_dates.extend(outcome.load_dates.iter().copied());
            outcome.conflicts.clear(); // Compute only AFTER all peers/targets have converged.
            outcomes.insert(id, outcome);
        }
        for id in self.board_only {
            if outcomes.contains_key(&id) {
                continue;
            }
            let applied_gen: i64 = sqlx::query_scalar(
                "select applied_gen from porchlight.item_derivation where item_id=$1",
            )
            .bind(id)
            .fetch_one(&mut **self.tx)
            .await?;
            outcomes.insert(
                id,
                DeriveOutcome {
                    applied_gen,
                    conflicts: Vec::new(),
                    load_dates: Vec::new(),
                },
            );
        }
        // s8b: recompute final loads after all peers/targets have converged.
        let unheld: BTreeSet<_> = load_dates
            .iter()
            .map(|(m, _)| *m)
            .filter(|m| !self.members.contains(m))
            .collect();
        crate::planning::recompute_load_dates(self.tx, &self.members, &load_dates, now).await?;
        crate::planning::enqueue_unheld_load(self.tx, &unheld.into_iter().collect::<Vec<_>>())
            .await?;
        let mut result = Vec::new();
        for (id, mut derived) in outcomes {
            derived.conflicts = derive::conflicts(self.tx, id, &self.members).await?;
            derived
                .conflicts
                .retain(|c| c.other_item_id.is_none_or(|id| !poisoned.contains(&id)));
            result.push(WriteOutcome {
                item_id: id,
                derived,
            });
        }
        sqlx::query("select set_config('porchlight.write_scope','finished',true)")
            .execute(&mut **self.tx)
            .await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[sqlx::test(migrator = "crate::MIGRATOR")]
    async fn acquisition_churn_is_capped_and_releases_attempt_locks(pool: sqlx::PgPool) {
        let h = crate::testing::seed_household(&pool).await.unwrap();
        let id = crate::testing::insert_appointment(&pool, &h).await.unwrap();
        let app = crate::testing::pool_as(&pool, crate::testing::Role::App, None)
            .await
            .unwrap();
        let mut tx = crate::tx::begin_as(&app, &crate::tx::Actor::system())
            .await
            .unwrap();
        let mut attempts = 0;
        let result = WriteScope::begin_checked(
            &mut tx,
            h.id,
            ScopeRequest {
                items: vec![id],
                ..Default::default()
            },
            Utc::now(),
            || {
                attempts += 1;
                false
            },
        )
        .await;
        assert!(matches!(result, Err(Error::RetryableScope)));
        assert_eq!(attempts, 3);
        let mut probe = pool.begin().await.unwrap(); // tx-guard: allow (test)
        sqlx::query("select id from porchlight.items where id=$1 for update nowait")
            .bind(id)
            .fetch_one(&mut *probe)
            .await
            .unwrap();
    }
}

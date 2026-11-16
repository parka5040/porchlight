//! Resource-exact coverage, checked independently before and after delayed workers.
use super::*;
mod mutations;
mod regressions;
mod writers;
pub(super) use writers::mutate_extra;

#[derive(Debug, Clone, PartialEq, Eq)]
enum FailureKind {
    Bundle,
    Notification,
    Settings(&'static str),
    Member,
    PrivateHint,
    Reset,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CoverageFailure {
    seed: u64,
    member: usize,
    phase: String,
    kind: FailureKind,
    id: Uuid,
}
impl std::fmt::Display for CoverageFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "seed={} member={} phase={} kind={:?} id={}",
            self.seed, self.member, self.phase, self.kind, self.id
        )
    }
}
fn failure(m: &Model, member: usize, phase: &str, kind: FailureKind, id: Uuid) -> CoverageFailure {
    CoverageFailure {
        seed: m.seed,
        member,
        phase: phase.into(),
        kind,
        id,
    }
}

fn rows(value: &Value) -> BTreeMap<Uuid, &Value> {
    value["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (id(v), v))
        .collect()
}
fn changed<'a>(before: &'a Value, after: &'a Value) -> Vec<Uuid> {
    let a = rows(before);
    let b = rows(after);
    a.keys()
        .chain(b.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|k| a.get(k) != b.get(k))
        .collect()
}
fn verify(
    m: &Model,
    member: usize,
    op: &str,
    before: &View,
    after: &View,
    frames: &[Value],
    reset: bool,
) -> Result<(), CoverageFailure> {
    let fail = |kind, id| failure(m, member, op, kind, id);
    let mut named = BTreeSet::new();
    for f in frames {
        let key = if f["entity_type"] == "item" {
            f["entity_id"].as_str()
        } else {
            f["item_id"].as_str()
        };
        if let Some(key) = key {
            let key: Uuid = key.parse().unwrap();
            named.insert(key);
            if !after.bundles.contains_key(&key)
                && !(f["evicted"] == true && m.seen[member].contains(&key))
            {
                return Err(fail(FailureKind::PrivateHint, key));
            }
        }
    }
    if reset {
        return Ok(());
    }
    for key in before
        .bundles
        .keys()
        .chain(after.bundles.keys())
        .copied()
        .collect::<BTreeSet<_>>()
    {
        if before.bundles.get(&key) != after.bundles.get(&key) && !named.contains(&key) {
            return Err(fail(FailureKind::Bundle, key));
        }
    }
    for key in changed(&before.inbox, &after.inbox) {
        if !frames.iter().any(|f| {
            f["entity_type"] == "notification"
                && f["entity_id"] == key.to_string()
                && f["kind"].as_str().unwrap().starts_with("notification.")
        }) {
            return Err(fail(FailureKind::Notification, key));
        }
    }
    for (index, resource) in ["member_settings", "notification_prefs", "availability"]
        .iter()
        .enumerate()
    {
        if before.settings[index] != after.settings[index]
            && !frames.iter().any(|f| {
                f["kind"] == "settings.changed"
                    && f["entity_type"] == *resource
                    && f["entity_id"] == m.members[member].to_string()
            })
        {
            return Err(fail(FailureKind::Settings(resource), m.members[member]));
        }
    }
    let a = rows(&before.settings[3]);
    let b = rows(&after.settings[3]);
    for key in changed(&before.settings[3], &after.settings[3]) {
        if frames.iter().any(|f| {
            f["kind"] == "member.changed"
                && f["entity_type"] == "member"
                && f["entity_id"] == key.to_string()
        }) {
            continue;
        }
        // Item/derived replacements also refresh caller pending flags. No other
        // profile change can be covered by an item tickle.
        let mut old = a.get(&key).copied().cloned().unwrap_or(Value::Null);
        let mut new = b.get(&key).copied().cloned().unwrap_or(Value::Null);
        for v in [&mut old, &mut new] {
            if let Some(v) = v.as_object_mut() {
                v.remove("pending_derivation");
            }
        }
        let pending_named = named.iter().any(|id| {
            let old = before.bundles.get(id).map(|v| &v["items"][0]);
            let new = after.bundles.get(id).map(|v| &v["items"][0]);
            old.map(|v| &v["pending_derivation"]) != new.map(|v| &v["pending_derivation"])
                && old.into_iter().chain(new).any(|v| {
                    v["owner_id"] == key.to_string()
                        || v["subject_member_id"] == key.to_string()
                        || v["assignees"]
                            .as_array()
                            .is_some_and(|ids| ids.contains(&json!(key)))
                })
        });
        if old != new || !pending_named {
            return Err(fail(FailureKind::Member, key));
        }
    }
    Ok(())
}

async fn phase(
    m: &mut Model,
    op: &str,
    before: &[View; 2],
    streams: &mut [TestStream; 2],
) -> Result<(), CoverageFailure> {
    // This barrier is consumed ONLY as a transport fence. received() excludes it
    // before returning frames, even for member/settings assertions.
    sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id) values($1,'member.changed','member',$2)")
        .bind(m.s.init.household_id).bind(Uuid::now_v7()).execute(&m.pool).await.unwrap();
    while porchlight_db::outbox::assign_stream_seqs(&m.pool, 500)
        .await
        .unwrap()
        .is_some()
    {}
    let barrier = porchlight_db::stream::maximum(&m.pool).await.unwrap();
    for (member, stream) in streams.iter_mut().enumerate() {
        let after = m.view(member).await;
        m.seen[member].extend(
            before[member]
                .bundles
                .keys()
                .chain(after.bundles.keys())
                .copied(),
        );
        let (frames, reset) = m.received(stream, barrier).await;
        if matches!(
            op,
            "household_zone/source" | "member_delete/source" | "role/source"
        ) && !reset
        {
            return Err(failure(
                m,
                member,
                op,
                FailureKind::Reset,
                m.s.init.household_id,
            ));
        }
        verify(m, member, op, &before[member], &after, &frames, reset)?;
    }
    Ok(())
}

pub(super) async fn checked_step(m: &mut Model, op: &str) -> Result<(), CoverageFailure> {
    if op == "horizon" {
        *m.clock.lock().unwrap() = now() + Days::days(120);
    }
    if op == "dst" {
        *m.clock.lock().unwrap() = now() + Days::hours(2);
    }
    m.drain().await;
    let before = [m.view(0).await, m.view(1).await];
    let cursor = porchlight_db::stream::maximum(&m.pool).await.unwrap();
    let mut streams = [
        open(&m.s, &m.devices[0], true, cursor).await,
        open(&m.s, &m.devices[1], true, cursor).await,
    ];
    m.mutate(op).await;
    if op == "member_zone_source" {
        let pending: bool = sqlx::query_scalar(
            "select requested_gen>applied_gen from porchlight.item_derivation where item_id=$1",
        )
        .bind(m.items[0])
        .fetch_one(&m.pool)
        .await
        .unwrap();
        assert!(
            pending,
            "source observation missed pending state: request-local drain already ran"
        );
    }
    phase(m, &format!("{op}/source"), &before, &mut streams).await?;
    drop(streams);
    // Background derivation is disabled. Only now allow the worker to observe
    // committed source state, then check its own committed projection changes.
    let before = [m.view(0).await, m.view(1).await];
    let cursor = porchlight_db::stream::maximum(&m.pool).await.unwrap();
    let mut streams = [
        open(&m.s, &m.devices[0], true, cursor).await,
        open(&m.s, &m.devices[1], true, cursor).await,
    ];
    m.drain().await;
    phase(m, &format!("{op}/worker"), &before, &mut streams).await
}

#[sqlx::test(migrations = "../db/migrations")]
async fn resource_checks_reject_barriers_and_unrelated_announcements(pool: PgPool) {
    let m = fixture(pool, 1).await;
    let key = Uuid::now_v7();
    let before = View {
        bundles: BTreeMap::new(),
        inbox: json!({"items":[]}),
        settings: json!([{}, {}, {}, {"items":[]}]),
    };
    let mut after = View {
        bundles: BTreeMap::new(),
        inbox: json!({"items":[{"id":key}]}),
        settings: before.settings.clone(),
    };
    let frames = vec![
        json!({"kind":"member.changed","entity_type":"member","entity_id":Uuid::now_v7()}),
        json!({"kind":"notification.updated","entity_type":"notification","entity_id":Uuid::now_v7()}),
    ];
    assert!(
        verify(&m, 0, "false-inbox", &before, &after, &frames, false)
            .unwrap_err()
            .kind
            == FailureKind::Notification
    );
    after.inbox = before.inbox.clone();
    for index in 0..3 {
        after.settings = before.settings.clone();
        after.settings[index] = json!({"changed":true});
        assert!(
            verify(&m, 0, "false-settings", &before, &after, &frames, false)
                .unwrap_err()
                .kind
                == FailureKind::Settings(
                    ["member_settings", "notification_prefs", "availability"][index]
                )
        );
    }
    m.s.stop().await;
}

pub(super) async fn fixture(pool: PgPool, seed: u64) -> Model {
    let clock = std::sync::Arc::new(std::sync::Mutex::new(now()));
    let read_clock = clock.clone();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || *read_clock.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let user = trust::user_named(&pool, &s, "member", "Sam Rivera").await.1;
    let members = [s.init.admin_member_id, user];
    let devices = [
        s.session_device(members[0], Platform::Desktop).await,
        s.session_device(members[1], Platform::Desktop).await,
    ];
    Model {
        s,
        pool,
        devices,
        members,
        seed,
        random: seed.max(1),
        clock,
        items: Vec::new(),
        list: Uuid::nil(),
        entry: Uuid::nil(),
        comment: Uuid::nil(),
        attachment: Uuid::nil(),
        proposals: Vec::new(),
        seen: Default::default(),
    }
}

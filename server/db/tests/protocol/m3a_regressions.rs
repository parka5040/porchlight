use chrono::{DateTime, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;

fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

async fn carry(
    pool: &PgPool,
    start: &str,
) -> (sqlx::Transaction<'static, sqlx::Postgres>, uuid::Uuid) {
    carry_mode(pool, start, false).await
}

async fn carry_mode(
    pool: &PgPool,
    start: &str,
    completion: bool,
) -> (sqlx::Transaction<'static, sqlx::Postgres>, uuid::Uuid) {
    carry_series(pool, start, completion, 3).await
}

async fn carry_series(
    pool: &PgPool,
    start: &str,
    completion: bool,
    count: u32,
) -> (sqlx::Transaction<'static, sqlx::Postgres>, uuid::Uuid) {
    let h = seed_household(pool).await.unwrap();
    let app = pool_as(pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Carry identity".into(),
        timing: core::SeriesInput {
            kind: core::Kind::Deadline,
            tzid: Some("UTC".into()),
            due_at: Some(at("2026-01-01T12:00:00Z")),
            rrule: (!completion).then(|| format!("FREQ=DAILY;COUNT={count}")),
            recurrence_mode: Some(if completion {
                core::RecurrenceMode::Completion
            } else {
                core::RecurrenceMode::Calendar
            }),
            completion_interval: completion.then_some(core::PgInterval {
                days: 1,
                ..Default::default()
            }),
            missed_policy: core::MissedPolicy::CarryOver,
            ..Default::default()
        },
        ..Default::default()
    };
    let out = items::create(&mut tx, h.id, &fields, &Default::default(), at(start))
        .await
        .unwrap();
    (tx, out.item_id)
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn carry_over_keeps_doing_identity_when_new_boundaries_pass(pool: PgPool) {
    let (mut tx, id) = carry(&pool, "2026-01-01T13:00:00Z").await;
    sqlx::query("update porchlight.item_occurrences set status='doing' where item_id=$1 and seq=0")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    derive::derive_item(&mut tx, id, at("2026-01-04T13:00:00Z"))
        .await
        .unwrap();
    let states: Vec<String> = sqlx::query_scalar(
        "select disposition from porchlight.item_occurrences where item_id=$1 order by seq",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        states,
        ["overdue", "expired", "expired"],
        "the already carried doing row must persist"
    );
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn carry_over_completion_never_resurrects_superseded_backlog(pool: PgPool) {
    let (mut tx, id) = carry(&pool, "2026-01-04T13:00:00Z").await;
    for seq in [2, 1, 0] {
        sqlx::query("update porchlight.item_occurrences set status='done', completed_at='2026-01-04T13:00:00Z', completed_by=(select owner_id from porchlight.items where id=$1) where item_id=$1 and seq=$2").bind(id).bind(seq).execute(&mut *tx).await.unwrap();
        derive::derive_item(&mut tx, id, at("2026-01-04T13:00:00Z"))
            .await
            .unwrap();
        let overdue: i64 = sqlx::query_scalar("select count(*) from porchlight.item_occurrences where item_id=$1 and status in ('todo','doing','backlog') and disposition='overdue'").bind(id).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            overdue, 0,
            "completing seq {seq} must not revive expired rows"
        );
    }
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn rescheduled_carried_row_preserves_identity_and_agenda(pool: PgPool) {
    for refreshed in [false, true] {
        for transition in [false, true] {
            for close_early in [false, true] {
                let (mut tx, id) = carry(&pool, "2026-01-01T13:00:00Z").await;
                let moved = at("2026-01-06T12:00:00Z");
                if refreshed {
                    derive::derive_item(&mut tx, id, at("2026-01-04T13:00:00Z"))
                        .await
                        .unwrap();
                }
                sqlx::query("update porchlight.item_occurrences set override_local_due=$2 where item_id=$1 and seq=0")
                    .bind(id).bind(moved.naive_utc()).execute(&mut *tx).await.unwrap();
                derive::derive_item(&mut tx, id, at("2026-01-04T13:00:00Z"))
                    .await
                    .unwrap();
                assert_eq!(
                    states(&mut tx, id).await,
                    ["open", "expired", "expired"],
                    "reschedule must not resurrect superseded peers (refresh={refreshed})"
                );
                if transition {
                    for policy in ["skip", "carry_over", "stack", "carry_over"] {
                        sqlx::query("update porchlight.items set missed_policy=$2 where id=$1")
                            .bind(id)
                            .bind(policy)
                            .execute(&mut *tx)
                            .await
                            .unwrap();
                        derive::derive_item(&mut tx, id, at("2026-01-04T13:00:00Z"))
                            .await
                            .unwrap();
                        if policy == "carry_over" {
                            assert_eq!(states(&mut tx, id).await, ["open", "expired", "expired"]);
                        }
                    }
                }
                if close_early {
                    sqlx::query("update porchlight.item_occurrences set status='done',completed_at='2026-01-05T13:00:00Z',completed_by=(select owner_id from porchlight.items where id=$1) where item_id=$1 and seq=0")
                        .bind(id).execute(&mut *tx).await.unwrap();
                }
                derive::derive_item(&mut tx, id, at("2026-01-07T13:00:00Z"))
                    .await
                    .unwrap();
                assert_eq!(
                    states(&mut tx, id).await,
                    [
                        if close_early { "open" } else { "overdue" },
                        "expired",
                        "expired"
                    ]
                );
                if !close_early {
                    let agenda_seqs: Vec<i32> = sqlx::query_scalar("select distinct o.seq from porchlight.member_agenda a join porchlight.item_occurrences o on o.id=a.occurrence_id where o.item_id=$1")
                        .bind(id).fetch_all(&mut *tx).await.unwrap();
                    assert_eq!(
                        agenda_seqs,
                        [0],
                        "moved carried row must remain visible in agenda"
                    );
                }
                tx.rollback().await.unwrap();
            }
        }
    }
}

// R63 probes P1 (NB-1) and P3 through the real derive path, with later siblings present.
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn later_sibling_moves_and_ties_keep_one_carried_row(pool: PgPool) {
    let (mut tx, id) = carry_series(&pool, "2026-01-01T13:00:00Z", false, 7).await;
    sqlx::query("update porchlight.item_occurrences set status='doing' where item_id=$1 and seq=0")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let carried = [
        "overdue", "expired", "expired", "open", "open", "open", "open",
    ];
    assert_eq!(
        derive_states(&mut tx, id, "2026-01-03T13:00:00Z").await,
        carried
    );
    // P1: postpone the still-future 01-04 instance by twelve hours.
    let set_due =
        "update porchlight.item_occurrences set override_local_due=$3 where item_id=$1 and seq=$2";
    sqlx::query(set_due)
        .bind(id)
        .bind(3)
        .bind(at("2026-01-05T00:00:00Z").naive_utc())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        derive_states(&mut tx, id, "2026-01-03T13:00:00Z").await,
        carried
    );
    assert_eq!(
        derive_states(&mut tx, id, "2026-01-04T18:00:00Z").await,
        carried,
        "its generated time passed: carried doing work stays overdue"
    );
    assert_eq!(
        derive_states(&mut tx, id, "2026-01-05T06:00:00Z").await,
        [
            "overdue", "expired", "expired", "expired", "open", "open", "open"
        ],
        "its new time passed: younger missed work behind the carried row"
    );
    // P3: move the carried row onto the 01-06 sibling's boundary.
    sqlx::query(set_due)
        .bind(id)
        .bind(0)
        .bind(at("2026-01-06T12:00:00Z").naive_utc())
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        derive_states(&mut tx, id, "2026-01-05T06:00:00Z").await,
        [
            "open", "expired", "expired", "expired", "open", "open", "open"
        ]
    );
    assert_eq!(
        derive_states(&mut tx, id, "2026-01-06T13:00:00Z").await,
        [
            "overdue", "expired", "expired", "expired", "expired", "expired", "open"
        ],
        "the tie keeps the moved carried row"
    );
    let agenda_seqs: Vec<i32> = sqlx::query_scalar("select distinct o.seq from porchlight.member_agenda a join porchlight.item_occurrences o on o.id=a.occurrence_id where o.item_id=$1 and o.seq<6")
        .bind(id).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(agenda_seqs, [0], "the moved doing row stays visible");
    tx.rollback().await.unwrap();
}

async fn derive_states(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    id: uuid::Uuid,
    now: &str,
) -> Vec<String> {
    derive::derive_item(tx, id, at(now)).await.unwrap();
    states(tx, id).await
}

async fn states(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, id: uuid::Uuid) -> Vec<String> {
    sqlx::query_scalar(
        "select disposition from porchlight.item_occurrences where item_id=$1 order by seq",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn policy_transitions_keep_calendar_visible(pool: PgPool) {
    transitions(&pool, false).await;
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn policy_transitions_keep_completion_visible(pool: PgPool) {
    transitions(&pool, true).await;
}
async fn transitions(pool: &PgPool, completion: bool) {
    {
        let (mut tx, id) = carry_mode(pool, "2026-01-04T13:00:00Z", completion).await;
        for policy in [
            "skip",
            "carry_over",
            "skip",
            "carry_over",
            "stack",
            "carry_over",
        ] {
            sqlx::query("update porchlight.items set missed_policy=$2 where id=$1")
                .bind(id)
                .bind(policy)
                .execute(&mut *tx)
                .await
                .unwrap();
            derive::derive_item(&mut tx, id, at("2026-01-04T13:00:00Z"))
                .await
                .unwrap();
            let overdue: i64 = sqlx::query_scalar("select count(*) from porchlight.item_occurrences where item_id=$1 and status in ('todo','doing','backlog') and disposition='overdue'").bind(id).fetch_one(&mut *tx).await.unwrap();
            if policy == "carry_over" {
                assert_eq!(
                    overdue, 1,
                    "completion={completion}: transition into carry_over must be visible"
                );
            }
            if completion {
                let open: i64 = sqlx::query_scalar("select count(*) from porchlight.item_occurrences where item_id=$1 and status in ('todo','doing','backlog')").bind(id).fetch_one(&mut *tx).await.unwrap();
                assert_eq!(open, 1, "exactly one completion successor");
            }
        }
        tx.rollback().await.unwrap();
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn completion_of_carried_row_covers_unrefreshed_peers(pool: PgPool) {
    for refreshed in [false, true] {
        let (mut tx, id) = carry(&pool, "2026-01-01T13:00:00Z").await;
        let closed = at("2026-01-04T13:00:00Z");
        if refreshed {
            derive::derive_item(&mut tx, id, closed).await.unwrap();
        }
        sqlx::query("update porchlight.item_occurrences set status='done',completed_at=$2,completed_by=(select owner_id from porchlight.items where id=$1) where item_id=$1 and seq=0").bind(id).bind(closed).execute(&mut *tx).await.unwrap();
        derive::derive_item(&mut tx, id, closed).await.unwrap();
        let overdue: i64 = sqlx::query_scalar("select count(*) from porchlight.item_occurrences where item_id=$1 and status in ('todo','doing','backlog') and disposition='overdue'").bind(id).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            overdue, 0,
            "refresh={refreshed}: missed before closure must not be carried"
        );
        tx.rollback().await.unwrap();
    }
}

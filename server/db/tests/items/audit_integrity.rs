use chrono::{DateTime, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    items::{self, ItemWrite, ScopeRequest, WriteScope},
    planning,
    testing::seed_household,
};
use sqlx::PgPool;

fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn split_recomputes_old_load_dates(pool: PgPool) {
    for move_date in [false, true] {
        let h = seed_household(&pool).await.unwrap();
        let now = at("2026-10-05T10:00:00Z");
        sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) select $1,$2,d,'09:00','10:00' from generate_series(0,6) d")
            .bind(h.id).bind(h.super_admin).execute(&pool).await.unwrap();
        let fields = ItemWrite {
            owner_id: h.super_admin,
            title: "Daily work".into(),
            est_minutes: Some(90),
            timing: core::SeriesInput {
                kind: core::Kind::Deadline,
                tzid: Some("America/New_York".into()),
                due_at: Some(at("2026-10-05T12:00:00Z")),
                rrule: Some("FREQ=DAILY;COUNT=3".into()),
                recurrence_mode: Some(core::RecurrenceMode::Calendar),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut tx = pool.begin().await.unwrap();
        let series = items::create(&mut tx, h.id, &fields, &Default::default(), now)
            .await
            .unwrap()
            .item_id;
        tx.commit().await.unwrap();
        let mut other = fields.clone();
        other.est_minutes = Some(20);
        other.timing.due_at = Some(at("2026-10-06T12:00:00Z"));
        other.timing.rrule = None;
        other.timing.recurrence_mode = None;
        let mut tx = pool.begin().await.unwrap();
        let standalone = items::create(&mut tx, h.id, &other, &Default::default(), now)
            .await
            .unwrap()
            .item_id;
        tx.commit().await.unwrap();
        let before: i64 = sqlx::query_scalar("select planned_minutes from porchlight.member_daily_load where member_id=$1 and date='2026-10-06'")
            .bind(h.super_admin).fetch_one(&pool).await.unwrap();
        assert_eq!(before, 110);
        let pending: bool = sqlx::query_scalar(
            "select requested_gen>applied_gen from porchlight.member_load_jobs where member_id=$1",
        )
        .bind(h.super_admin)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!pending, "regression must not rely on full refresh");
        let mut tx = pool.begin().await.unwrap();
        let mut scope = WriteScope::begin(
            &mut tx,
            h.id,
            ScopeRequest {
                items: vec![series],
                splits: vec![(series, 1)],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let row = scope.item(series).await.unwrap();
        let (_, tail) = scope.split(series, row.version, 1).await.unwrap();
        let row = scope.item(tail).await.unwrap();
        let mut fields = ItemWrite::from(&row);
        if move_date {
            fields.timing.due_at = Some(at("2026-10-08T12:00:00Z"));
        } else {
            fields.est_minutes = None;
        }
        scope.update(tail, row.version, &fields).await.unwrap();
        scope.finish(now).await.unwrap();
        let conflicts = planning::load_conflicts(&mut tx, &[standalone], Some(&[h.super_admin]))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let after: i64 = sqlx::query_scalar("select planned_minutes from porchlight.member_daily_load where member_id=$1 and date='2026-10-06'")
            .bind(h.super_admin).fetch_one(&pool).await.unwrap();
        assert_eq!(after, 20, "old date load must clear: move_date={move_date}");
        assert!(
            conflicts.is_empty(),
            "standalone task must lose its load warning"
        );
        let cleared: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where recipient_member_id=$1 and kind='load_conflict' and payload->>'date'='2026-10-06' and payload->>'conflicting'='false'")
            .bind(h.super_admin).fetch_one(&pool).await.unwrap();
        assert_eq!(cleared, 1);
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn sql_ordering_keys_match_core(pool: PgPool) {
    use porchlight_core::ordering::{self, OrderingError};
    use porchlight_db::testing::{Role, insert_appointment, pool_as};
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let occurrence:uuid::Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0) returning id")
        .bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let list:uuid::Uuid = sqlx::query_scalar("insert into porchlight.lists(household_id,owner_id,name) values($1,$2,'Ordered list') returning id")
        .bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    let entry:uuid::Uuid = sqlx::query_scalar("insert into porchlight.list_entries(household_id,list_id,label) values($1,$2,'Entry') returning id")
        .bind(h.id).bind(list).fetch_one(&pool).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:audit-integrity"))
        .await
        .unwrap();
    for (id, sql) in [
        (
            occurrence,
            "update porchlight.item_occurrences set board_position=$2 where id=$1",
        ),
        (
            entry,
            "update porchlight.list_entries set position=$2 where id=$1",
        ),
    ] {
        for invalid in ["a0", "a", "80ff", "", "AB80", "é80"] {
            let before:(i64,i64) = sqlx::query_as("select (select count(*) from porchlight.outbox),(select coalesce(sum(version),0)::bigint from porchlight.item_occurrences)")
                .fetch_one(&pool).await.unwrap();
            let result = sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(id)
                .bind(invalid)
                .execute(&agent)
                .await;
            assert!(result.is_err(), "SQL accepted noncanonical key {invalid}");
            assert_eq!(
                result
                    .unwrap_err()
                    .as_database_error()
                    .unwrap()
                    .code()
                    .as_deref(),
                Some("23514")
            );
            let after:(i64,i64) = sqlx::query_as("select (select count(*) from porchlight.outbox),(select coalesce(sum(version),0)::bigint from porchlight.item_occurrences)")
                .fetch_one(&pool).await.unwrap();
            assert_eq!(before, after, "invalid writes must be atomic");
        }
        let mut samples = vec![
            "80".to_owned(),
            format!("{}80", "00".repeat(31)),
            format!("{}80", "ff".repeat(31)),
            format!("{}80", "00".repeat(32)),
        ];
        for byte in 0..=255 {
            samples.extend([
                format!("{byte:02x}"),
                format!("{byte:02x}80"),
                format!("80{byte:02x}"),
            ]);
        }
        for key in samples {
            let core_accepts =
                ordering::key_between(Some(&key), Some(&key)) == Err(OrderingError::InvalidBounds);
            let sql_accepts = sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(id)
                .bind(&key)
                .execute(&agent)
                .await
                .is_ok();
            assert_eq!(sql_accepts, core_accepts, "SQL/core key parity: {key}");
        }
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn canonical_migration_preserves_legacy_order_and_restarts(pool: PgPool) {
    use porchlight_db::testing::insert_appointment;
    let h = seed_household(&pool).await.unwrap();
    sqlx::raw_sql("alter table porchlight.item_occurrences drop constraint item_occurrences_board_position_canonical; alter table porchlight.list_entries drop constraint list_entries_position_canonical;")
        .execute(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,board_position) select $1,$2,n,case when n%2=0 then 'a' else '80' end from generate_series(0,1004) n")
        .bind(h.id).bind(item).execute(&pool).await.unwrap();
    let list:uuid::Uuid = sqlx::query_scalar("insert into porchlight.lists(household_id,owner_id,name) values($1,$2,'Legacy list') returning id")
        .bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.list_entries(household_id,list_id,label,position,archived_at) select $1,$2,'Entry',case when n%2=0 then 'a0' else '80' end,case when n%3=0 then now() end from generate_series(0,1004) n")
        .bind(h.id).bind(list).execute(&pool).await.unwrap();
    let before_board:Vec<uuid::Uuid> = sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 order by board_position collate \"C\",id")
        .bind(item).fetch_all(&pool).await.unwrap();
    let before_list:Vec<uuid::Uuid> = sqlx::query_scalar("select id from porchlight.list_entries where list_id=$1 order by position collate \"C\",id")
        .bind(list).fetch_all(&pool).await.unwrap();
    let migration = porchlight_db::MIGRATOR
        .iter()
        .find(|m| m.version == 67)
        .unwrap();
    assert!(
        migration.no_tx,
        "validation must not retain constraint-add exclusive locks"
    );
    let mut conn = pool.acquire().await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(migration.sql.as_ref()))
        .execute(&mut *conn)
        .await
        .unwrap();
    let after_board:Vec<uuid::Uuid> = sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 order by board_position collate \"C\",id")
        .bind(item).fetch_all(&mut *conn).await.unwrap();
    let after_list:Vec<uuid::Uuid> = sqlx::query_scalar("select id from porchlight.list_entries where list_id=$1 order by position collate \"C\",id")
        .bind(list).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(before_board, after_board);
    assert_eq!(before_list, after_list);
    let moved: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where item_id=$1 and board_moved",
    )
    .bind(item)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(moved, 0, "normalization is not an explicit board move");
    let versions: i64 = sqlx::query_scalar(
        "select sum(version)::bigint from porchlight.item_occurrences where item_id=$1",
    )
    .bind(item)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(migration.sql.as_ref()))
        .execute(&mut *conn)
        .await
        .unwrap();
    let restarted_versions: i64 = sqlx::query_scalar(
        "select sum(version)::bigint from porchlight.item_occurrences where item_id=$1",
    )
    .bind(item)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        versions, restarted_versions,
        "completed repair is restartable without rewriting valid rows"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn board_expands_exhausted_canonical_run(pool: PgPool) {
    use porchlight_db::testing::insert_appointment;
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let mut keys: Vec<_> = (1..=4)
        .map(|i| format!("{}{i:02x}80", "00".repeat(30)))
        .collect();
    keys.extend(["80".to_owned(), "f080".to_owned()]);
    let now = Utc::now();
    let mut ids = Vec::new();
    for (seq, key) in keys.iter().enumerate() {
        let id:uuid::Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at,board_position) values($1,$2,$3,'done',$4,$5) returning id")
            .bind(h.id).bind(item).bind(seq as i32).bind(now).bind(key).fetch_one(&pool).await.unwrap();
        ids.push(id);
    }
    let mut tx = pool.begin().await.unwrap();
    items::move_on_board(&mut tx, h.id, ids[4], 1, Some(ids[1]), Some(ids[2]), now)
        .await
        .expect("dense legal board must widen its exhausted run");
    tx.commit().await.unwrap();
    let ordered:Vec<uuid::Uuid> = sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 order by board_position collate \"C\",id")
        .bind(item).fetch_all(&pool).await.unwrap();
    assert_eq!(
        ordered,
        vec![ids[0], ids[1], ids[4], ids[2], ids[3], ids[5]]
    );
    let unrelated: (String, i32) = sqlx::query_as(
        "select board_position,version from porchlight.item_occurrences where id=$1",
    )
    .bind(ids[5])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unrelated, ("f080".to_owned(), 1));
    let moved: Vec<uuid::Uuid> = sqlx::query_scalar(
        "select id from porchlight.item_occurrences where item_id=$1 and board_moved",
    )
    .bind(item)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(moved, vec![ids[4]]);
}

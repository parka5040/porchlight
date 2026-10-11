use porchlight_core::{items::Rule, ordering};
use porchlight_db::testing::{Role, connect_as, insert_appointment, seed_household};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn schema_metadata_lineage_and_default(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let columns: i64 = sqlx::query_scalar("select count(*) from information_schema.columns where table_schema='porchlight' and table_name='items' and column_name in ('series_id','split_from_item_id','split_from_seq','nominal_local_anchor','nominal_wall_duration')").fetch_one(&pool).await.unwrap();
    assert_eq!(columns, 5);
    let position: String = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0) returning board_position").bind(h.id).bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(position, ordering::first_key());
    for query in [
        "update porchlight.items set split_from_item_id=id where id=$1",
        "update porchlight.items set all_day=true,recurrence_mode='calendar',rrule='FREQ=DAILY;COUNT=2',nominal_local_anchor='2026-10-12 14:00',nominal_wall_duration=3600000000 where id=$1",
        "update porchlight.items set nominal_wall_duration=1 where id=$1",
        "update porchlight.items set nominal_local_anchor='2026-10-12 14:00',nominal_wall_duration=0 where id=$1",
        "update porchlight.items set nominal_local_anchor='2026-10-12 14:00',nominal_wall_duration=3600000000 where id=$1",
    ] {
        assert!(
            sqlx::query(sqlx::AssertSqlSafe(query))
                .bind(id)
                .execute(&pool)
                .await
                .is_err()
        );
    }
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:schema"))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.scheduler_jobs")
        .fetch_one(&mut agent)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert!(
        sqlx::query(
            "insert into porchlight.scheduler_jobs(name,next_due_at) values('forbidden',now())"
        )
        .execute(&mut agent)
        .await
        .is_err()
    );
}

#[sqlx::test(migrations = false)]
async fn migration_backfills_every_invalid_key_in_stable_board_order(pool: PgPool) {
    let old = sqlx::migrate::Migrator::with_migrations(
        porchlight_db::MIGRATOR
            .iter()
            .filter(|m| m.version < 12)
            .cloned()
            .collect(),
    );
    // 0001 changes the database default search_path for NEW connections. Keep
    // both migration phases on the original session, where the tracking table
    // was created in public, rather than racing the pool's background connects.
    let mut migrator_connection = pool.acquire().await.unwrap();
    old.run(&mut *migrator_connection).await.unwrap();
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let inputs = [
        ("todo", "a0"),
        ("todo", "80"),
        ("doing", "801"),
        ("backlog", "aé"),
        ("todo", "AB80"),
        ("todo", "aa80"),
    ];
    for (seq, (status, key)) in inputs.iter().enumerate() {
        sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,status,board_position) values($1,$2,$3,$4,$5)").bind(h.id).bind(id).bind(seq as i32).bind(status).bind(key).execute(&pool).await.unwrap();
    }
    let order:Vec<uuid::Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences order by status,board_position collate \"C\",id").fetch_all(&pool).await.unwrap();
    porchlight_db::MIGRATOR
        .run(&mut *migrator_connection)
        .await
        .unwrap();
    let rows:Vec<(uuid::Uuid,String)>=sqlx::query_as("select id,board_position from porchlight.item_occurrences order by board_position collate \"C\",id").fetch_all(&pool).await.unwrap();
    assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), order);
    assert_eq!(
        rows.iter().map(|r| r.1.clone()).collect::<Vec<_>>(),
        ordering::rebalance(inputs.len())
    );
    for (_, key) in rows {
        assert!(ordering::key_after(&key).is_ok());
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn database_half_of_shared_rrule_corpus(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut accepted = 0;
    let mut regex_rejected = 0;
    let mut semantic_rejected = 0;
    for line in include_str!("../../../core/tests/fixtures/rrule_corpus.txt")
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        let (verdict, rule) = line.split_once(' ').unwrap();
        let regex:bool=sqlx::query_scalar("select $1::text ~ '^FREQ=(DAILY|WEEKLY|MONTHLY|YEARLY)(;(INTERVAL=[1-9][0-9]{0,2}|BYDAY=[+-]?[1-5]?(MO|TU|WE|TH|FR|SA|SU)(,[+-]?[1-5]?(MO|TU|WE|TH|FR|SA|SU))*|BYMONTHDAY=-?([1-9]|[12][0-9]|3[01])(,-?([1-9]|[12][0-9]|3[01]))*|COUNT=[1-9][0-9]{0,3}|UNTIL=[0-9]{8}(T[0-9]{6}Z?)?))*$'").bind(rule).fetch_one(&pool).await.unwrap();
        let result=sqlx::query("insert into porchlight.items(household_id,owner_id,title,kind,due_date,rrule,recurrence_mode) values($1,$2,'Corpus','todo','2026-01-01',$3,'calendar')").bind(h.id).bind(h.user).bind(rule).execute(&pool).await;
        assert_eq!(result.is_ok(), regex, "{rule}");
        if verdict == "accept" {
            assert!(result.is_ok(), "{rule}");
            accepted += 1;
        } else if regex {
            assert!(rule.parse::<Rule>().is_err());
            semantic_rejected += 1;
        } else {
            regex_rejected += 1;
        }
    }
    assert_eq!((accepted, regex_rejected, semantic_rejected), (12, 19, 19));
}

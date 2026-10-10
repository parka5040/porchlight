use porchlight_core::time::{self, STORAGE_ZONE_LINKS};
use porchlight_db::testing::seed_household;
use sqlx::{Executor, PgPool};

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn every_storage_name_passes_postgres_catalog_and_check(pool: PgPool) {
    let mut names = std::collections::BTreeSet::new();
    for &(_, target) in STORAGE_ZONE_LINKS {
        names.insert(target);
    }
    for zone in chrono_tz::TZ_VARIANTS {
        match time::storage_zone(zone.name()) {
            Ok((_, name)) => {
                names.insert(name);
            }
            Err(_) => assert_eq!(zone.name(), "Factory", "unexpected exception"),
        }
    }
    for name in names {
        let accepted:bool=sqlx::query_scalar("select porchlight.pl_valid_tzid($1) and exists(select 1 from pg_timezone_names where name=$1)").bind(name).fetch_one(&pool).await.unwrap();
        assert!(accepted, "storage target absent from PostgreSQL: {name}");
    }
}
fn migration_prefix() -> &'static str {
    include_str!("../migrations/0015_items_reads.sql")
        .split("-- One authoritative ACL")
        .next()
        .unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn draft_migration_normalizes_existing_rows_and_names_invalid_rows(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    // Simulate the old catalog validator before the draft's tightening.
    tx.execute("create or replace function porchlight.pl_valid_tzid(z text) returns boolean language sql stable as $$ select true $$").await.unwrap();
    sqlx::query("update porchlight.households set tzid='EST5EDT' where id=$1")
        .bind(h.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("update porchlight.household_members set tzid='GMT' where id=$1")
        .bind(h.user)
        .execute(&mut *tx)
        .await
        .unwrap();
    let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,tzid,due_at) values($1,$2,'old alias','deadline','fixed','Etc/UTC','2026-10-06T12:00:00Z') returning id").bind(h.id).bind(h.user).fetch_one(&mut *tx).await.unwrap();
    sqlx::raw_sql(migration_prefix())
        .execute(&mut *tx)
        .await
        .unwrap();
    let zones:(String,String,String)=sqlx::query_as("select h.tzid,m.tzid,i.tzid from porchlight.households h join porchlight.household_members m on m.household_id=h.id join porchlight.items i on i.household_id=h.id where m.id=$1 and i.id=$2").bind(h.user).bind(item).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(
        zones,
        ("America/New_York".into(), "UTC".into(), "UTC".into())
    );
    tx.rollback().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    tx.execute("create or replace function porchlight.pl_valid_tzid(z text) returns boolean language sql stable as $$ select true $$").await.unwrap();
    sqlx::query("update porchlight.household_members set tzid='Factory' where id=$1")
        .bind(h.user)
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = sqlx::raw_sql(migration_prefix())
        .execute(&mut *tx)
        .await
        .unwrap_err();
    let message = err.as_database_error().unwrap().message();
    assert!(
        message.contains("household_members")
            && message.contains(&h.user.to_string())
            && message.contains("Factory"),
        "{message}"
    );
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn migration_static_map_matches_every_core_link(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    let prefix = migration_prefix().replace("DROP FUNCTION pl_storage_tzid(text);", "");
    sqlx::raw_sql(sqlx::AssertSqlSafe(prefix.as_str()))
        .execute(&mut *tx)
        .await
        .unwrap();
    for &(link, target) in STORAGE_ZONE_LINKS {
        let normalized: String = sqlx::query_scalar("select porchlight.pl_storage_tzid($1)")
            .bind(link)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(normalized, target, "{link}");
    }
    tx.rollback().await.unwrap();
}

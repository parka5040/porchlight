use porchlight_db::{
    testing::seed_household,
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;

async fn item(pool: &PgPool, h: Uuid, owner: Uuid) -> Uuid {
    sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'Fixture','todo','household') returning id")
        .bind(h).bind(owner).fetch_one(pool).await.unwrap()
}
pub(super) async fn history(pool: &PgPool, h: Uuid, actor: Uuid, recipient: Uuid) {
    let mut tx = begin_as(
        pool,
        &Actor {
            member_id: Some(actor),
            ..Actor::system()
        },
    )
    .await
    .unwrap();
    // Thousands of both single-source and coalesced released rows.
    for index in 0..2000 {
        let n: Uuid = sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,entity_type,entity_id,window_start,release_at,released_at) values($1,$2,'comment','activity','member',$3,now(),now(),now()) returning id")
            .bind(h).bind(recipient).bind(actor).fetch_one(&mut *tx).await.unwrap();
        for _ in 0..(1 + index % 2) {
            let e: Uuid = sqlx::query_scalar("select porchlight.pl_emit_notification_event($1,$2,'comment','member',$3,'{}',null,null,null)")
                .bind(h).bind(recipient).bind(actor).fetch_one(&mut *tx).await.unwrap();
            sqlx::query("insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)")
                .bind(n).bind(e).execute(&mut *tx).await.unwrap();
        }
    }
    tx.commit().await.unwrap();
}
async fn extras(pool: &PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from porchlight.outbox where kind in ('notification.invalidated','item.invalidated','sync.reset')")
        .fetch_one(pool).await.unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn unrelated_member_edits_do_not_walk_actor_history(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    history(&pool, h.id, h.super_admin, h.user).await;
    sqlx::query("insert into porchlight.devices(household_id,member_id,label,platform) values($1,$2,'Fixture authority','desktop')")
        .bind(h.id).bind(h.super_admin).execute(&pool).await.unwrap();
    for sql in [
        "update porchlight.household_members set color='#ABCDEF' where id=$1",
        "update porchlight.household_members set tz_source='manual' where id=$1",
        "update porchlight.household_members set tzid='Pacific/Auckland' where id=$1",
        "update porchlight.household_members set tz_authority_device_id=(select id from porchlight.devices where member_id=$1 limit 1) where id=$1",
        "update porchlight.household_members set display_name=display_name where id=$1",
    ] {
        let before = extras(&pool).await;
        sqlx::query(sql)
            .bind(h.super_admin)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            extras(&pool).await,
            before,
            "unrelated member edit fanned out actor history"
        );
    }
    let before = extras(&pool).await;
    sqlx::query(
        "update porchlight.household_members set display_name='Renamed fixture' where id=$1",
    )
    .bind(h.super_admin)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        extras(&pool).await - before,
        201,
        "rename must be capped with reset"
    );
}
pub(super) async fn lineage(pool: &PgPool, h: Uuid, owner: Uuid, length: i32) -> Uuid {
    let root = item(pool, h, owner).await;
    sqlx::query("with keys as (select array_agg(gen_random_uuid()) as ids from generate_series(1,$4::int)) insert into porchlight.items(id,household_id,owner_id,title,kind,visibility,series_id,split_from_item_id,split_from_seq) select k.id,$1,$2,'Tail','todo','household',$3,case when k.n=1 then $3 else keys.ids[k.n-1] end,1 from keys,unnest(keys.ids) with ordinality k(id,n)")
        .bind(h).bind(owner).bind(root).bind(length).execute(pool).await.unwrap();
    root
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn bounded_lineage_work_and_reset_short_circuit(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    for length in [400, 800] {
        let root = lineage(&pool, h.id, h.super_admin, length).await;
        sqlx::query("analyze porchlight.items")
            .execute(&pool)
            .await
            .unwrap();
        // Explain the exact shipping function body; its pinned search_path
        // deliberately prevents SQL-function inlining into the outer EXPLAIN.
        let body: String = sqlx::query_scalar("select prosrc from pg_proc where oid='porchlight.pl_sync_descendants(uuid,uuid)'::regprocedure")
            .fetch_one(&pool).await.unwrap();
        let explain = format!(
            "explain (analyze, format json) {}",
            body.replace("p_household", "$1").replace("p_parent", "$2")
        );
        let plan: serde_json::Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain))
            .bind(h.id)
            .bind(root)
            .fetch_one(&pool)
            .await
            .unwrap();
        fn recursive_rows(v: &serde_json::Value) -> Option<u64> {
            if v["Node Type"] == "Recursive Union" {
                return v["Actual Rows"].as_f64().map(|n| n as u64);
            }
            v["Plans"].as_array()?.iter().find_map(recursive_rows)
        }
        let visited = recursive_rows(&plan[0]["Plan"]).unwrap_or_else(|| {
            panic!("EXPLAIN must expose recursive work, not just output rows: {plan}")
        });
        assert!(visited <= 201, "unbounded recursive discovery: {plan}");
        fn scanned_items(v: &serde_json::Value) -> f64 {
            let own = if v["Relation Name"] == "items" {
                (v["Actual Rows"].as_f64().unwrap()
                    + v["Rows Removed by Filter"].as_f64().unwrap_or(0.0))
                    * v["Actual Loops"].as_f64().unwrap()
            } else {
                0.0
            };
            own + v["Plans"]
                .as_array()
                .map(|plans| plans.iter().map(scanned_items).sum::<f64>())
                .unwrap_or(0.0)
        }
        let scanned = scanned_items(&plan[0]["Plan"]);
        assert!(
            scanned <= 402.0,
            "recursive discovery scanned the full household: {plan}"
        );
        eprintln!(
            "lineage length={length} recursive actual rows={visited} scanned item rows={scanned}"
        );
        // Instrument helper invocation, not just emitted rows. A reset must stop
        // repeated relationship parent bumps before the recursive query starts.
        sqlx::raw_sql("create sequence porchlight.fixture_lineage_calls; grant usage on sequence porchlight.fixture_lineage_calls to porchlight_owner; alter function porchlight.pl_sync_descendants(uuid,uuid) rename to fixture_descendants; create function porchlight.pl_sync_descendants(p_household uuid,p_parent uuid) returns table(id uuid,depth integer) language plpgsql set search_path=pg_catalog,porchlight,pg_temp as $$ begin perform nextval('porchlight.fixture_lineage_calls'); return query select * from porchlight.fixture_descendants(p_household,p_parent); end $$")
            .execute(&pool).await.unwrap();
        let mut tx = begin_as(&pool, &Actor::system()).await.unwrap();
        for _ in 0..10 {
            sqlx::query("update porchlight.items set updated_at=now() where id=$1")
                .bind(root)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        let calls: i64 =
            sqlx::query_scalar("select last_value from porchlight.fixture_lineage_calls")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            calls, 1,
            "reset did not short-circuit repeated lineage discovery"
        );
        let reset: bool=sqlx::query_scalar("select exists(select 1 from porchlight.outbox where kind='sync.reset' and household_id=$1)")
            .bind(h.id).fetch_one(&pool).await.unwrap();
        assert!(reset, "overflow left readable tail unnamed without reset");
        sqlx::raw_sql("drop function porchlight.pl_sync_descendants(uuid,uuid); alter function porchlight.fixture_descendants(uuid,uuid) rename to pl_sync_descendants; drop sequence porchlight.fixture_lineage_calls")
            .execute(&pool).await.unwrap();
        eprintln!("lineage length={length} repeated parent bumps=10 discovery calls={calls}");
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn routine_title_edit_does_not_invalidate_lineage(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let head = item(&pool, h.id, h.super_admin).await;
    let tail = item(&pool, h.id, h.super_admin).await;
    sqlx::query("update porchlight.items set series_id=$1,split_from_item_id=$1,split_from_seq=1 where id=$2")
        .bind(head).bind(tail).execute(&pool).await.unwrap();
    let before = extras(&pool).await;
    sqlx::query("update porchlight.items set title='Edited title' where id=$1")
        .bind(head)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        extras(&pool).await,
        before,
        "title-only edit traversed inherited collaboration lineage"
    );
}

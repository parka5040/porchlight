//! Reciprocal PAT / raw relationship contention: precise blocker probes, no timing sleeps.
use porchlight_api::testing::wait_blocked_behind;
use porchlight_db::{
    items,
    testing::{Role, connect_as, insert_appointment, pool_as, seed_household},
    tokens,
};
use sqlx::{Connection, PgPool};
use std::time::Duration;
use uuid::Uuid;
const BOUND: Duration = Duration::from_secs(15);
fn state(e: &sqlx::Error) -> Option<String> {
    e.as_database_error()
        .and_then(|d| d.code().map(|s| s.into_owned()))
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reciprocal_pat_revoke_never_waits_on_cross_target(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut ids = vec![];
    for n in 1..=2u8 {
        let id: Uuid = sqlx::query_scalar("insert into porchlight.api_tokens(household_id,owner_member_id,name,scopes,token_hash,expires_at) values($1,$2,'lock regression',ARRAY['admin'], $3, now()+interval '1 year') returning id")
            .bind(h.id).bind(h.super_admin).bind(vec![n;32]).fetch_one(&pool).await.unwrap();
        ids.push(id);
    }
    for reverse in [false, true].into_iter().cycle().take(40) {
        let (a, b) = if reverse {
            (ids[1], ids[0])
        } else {
            (ids[0], ids[1])
        };
        let mut ca = connect_as(&pool, Role::App, None).await.unwrap();
        let mut cb = connect_as(&pool, Role::App, None).await.unwrap();
        let mut ta = ca.begin().await.unwrap();
        let mut tb = cb.begin().await.unwrap();
        // Same own-PAT claims as IdemTx::revalidate.
        tokens::share_pat(&mut ta, a).await.unwrap().unwrap();
        tokens::share_pat(&mut tb, b).await.unwrap().unwrap();
        // Savepoints keep both own-PAT locks held even after one NOWAIT error.
        // Disable lock_timeout so a waiting implementation cannot masquerade as NOWAIT.
        for tx in [&mut ta, &mut tb] {
            sqlx::query("set local lock_timeout='0'")
                .execute(&mut **tx)
                .await
                .unwrap();
        }
        let barrier = tokio::sync::Barrier::new(2);
        let first = async {
            let mut attempt = sqlx::Acquire::begin(&mut ta).await.unwrap();
            barrier.wait().await;
            let result = tokens::revoke(&mut attempt, h.id, b).await;
            attempt.rollback().await.unwrap();
            result
        };
        let second = async {
            let mut attempt = sqlx::Acquire::begin(&mut tb).await.unwrap();
            barrier.wait().await;
            let result = tokens::revoke(&mut attempt, h.id, a).await;
            attempt.rollback().await.unwrap();
            result
        };
        let (one, two) = tokio::time::timeout(BOUND, async { tokio::join!(first, second) })
            .await
            .expect("bounded cross-revoke");
        let codes = [
            one.err().as_ref().and_then(state),
            two.err().as_ref().and_then(state),
        ];
        assert_eq!(
            codes,
            [Some("55P03".into()), Some("55P03".into())],
            "both cross-targets must be NOWAIT"
        );
        ta.rollback().await.unwrap();
        tb.rollback().await.unwrap();
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn raw_relationship_delete_never_waits_for_scope_parent(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("insert into porchlight.item_assignees values($1,$2,$3)")
        .bind(h.id)
        .bind(id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.item_viewers values($1,$2,$3)")
        .bind(h.id)
        .bind(id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)").bind(h.id).bind(id).execute(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    for table in ["item_assignees", "item_viewers", "item_reminders"] {
        let mut api = app.begin().await.unwrap();
        sqlx::query("set local lock_timeout='0'")
            .execute(&mut *api)
            .await
            .unwrap();
        let holder: i32 = sqlx::query_scalar("select pg_backend_pid()")
            .fetch_one(&mut *api)
            .await
            .unwrap();
        let mut scope = items::WriteScope::begin_at(
            &mut api,
            h.id,
            items::ScopeRequest {
                items: vec![id],
                members: vec![h.super_admin, h.user],
                ..Default::default()
            },
            "2026-10-06T12:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();
        let row = scope.item(id).await.unwrap();
        let mut raw = connect_as(&pool, Role::Agent, Some("agent:m3b-regression"))
            .await
            .unwrap();
        let mut tx = raw.begin().await.unwrap();
        sqlx::query("set local lock_timeout='0'")
            .execute(&mut *tx)
            .await
            .unwrap();
        let delete = format!("delete from porchlight.{table} where item_id=$1");
        let first = async {
            // Table comes only from the static relationship allowlist above.
            let r = sqlx::query(sqlx::AssertSqlSafe(delete.as_str()))
                .bind(id)
                .execute(&mut *tx)
                .await;
            tx.rollback().await.unwrap();
            r
        };
        tokio::pin!(first);
        let result = tokio::select! {
            result = &mut first => result,
            blocked = wait_blocked_behind(&pool,holder,1,BOUND) => {
                blocked.unwrap();
                // Execute the replacement child DELETE with the scope's parent locks held.
                drop(scope);
                let replacement = async {
                    let r = sqlx::query(sqlx::AssertSqlSafe(delete.as_str())).bind(id).execute(&mut *api).await;
                    api.rollback().await.unwrap();
                    r
                };
                let (one,two) = tokio::time::timeout(BOUND,async { tokio::join!(&mut first,replacement) }).await.expect("bounded relationship replacement");
                let codes = [one.as_ref().err().and_then(state),two.as_ref().err().map(|e|format!("{e:?}"))];
                assert!(!format!("{codes:?}").contains("40P01"),"relationship cycle: {codes:?}");
                panic!("raw DELETE waited for scope parent: {table}; results: {codes:?}");
            }
        };
        assert_eq!(
            result.as_ref().err().and_then(state).as_deref(),
            Some("55P03"),
            "{table} must reject busy parent"
        );
        // Failed raw statement releases child locks; an actual API replacement
        // DELETE now succeeds while holding the parent's lock.
        scope
            .replace_relationships(id, row.version, &items::Relationships::default())
            .await
            .unwrap();
        scope
            .finish("2026-10-06T12:00:00Z".parse().unwrap())
            .await
            .unwrap();
        api.rollback().await.unwrap();
    }
}

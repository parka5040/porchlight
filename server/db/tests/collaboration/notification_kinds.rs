use porchlight_db::{
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn registry_is_extensible_fk_checked_and_owner_writable_only(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let kinds: Vec<(String, bool)> = sqlx::query_as(
        "select kind,self_suppressed from porchlight.notification_kinds order by kind",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for (kind, expected) in [
        ("assigned", true),
        ("status_changed", true),
        ("completed", true),
        ("comment", true),
        ("mention", true),
        ("conflict", false),
        ("load_conflict", false),
        ("reminder", false),
        ("dependent_unassigned", true),
        ("derivation_error", false),
        ("suggestion", true),
        ("calendar_changed", true),
        ("feed_error", false),
    ] {
        assert_eq!(
            kinds.iter().find(|(k, _)| k == kind).map(|(_, flag)| *flag),
            Some(expected),
            "{kind}"
        );
    }
    // A subsequent migration can register data without replacing the producer ABI.
    let before: String = sqlx::query_scalar("select pg_get_functiondef('porchlight.pl_emit_notification_event(uuid,uuid,text,text,uuid,jsonb,uuid,uuid,text)'::regprocedure)").fetch_one(&pool).await.unwrap();
    let mut migration = pool.begin().await.unwrap();
    sqlx::query("set local role porchlight_owner")
        .execute(&mut *migration)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.notification_kinds(kind,self_suppressed,description) values ('later_business',true,'Later business'),('later_operational',false,'Later operational') on conflict(kind) do nothing").execute(&mut *migration).await.unwrap();
    migration.commit().await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:registry"))
        .await
        .unwrap();
    for p in [&app, &agent] {
        let n: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_kinds")
            .fetch_one(p)
            .await
            .unwrap();
        assert_eq!(n, kinds.len() as i64 + 2);
        for sql in [
            "insert into porchlight.notification_kinds values ('forbidden',false,'Forbidden')",
            "update porchlight.notification_kinds set self_suppressed=false where kind='assigned'",
            "delete from porchlight.notification_kinds where kind='assigned'",
        ] {
            let error = sqlx::query(sql).execute(p).await.unwrap_err();
            assert_eq!(
                error.as_database_error().unwrap().code().as_deref(),
                Some("42501")
            );
        }
    }
    let actor = Actor {
        member_id: Some(h.user),
        ..Actor::system()
    };
    let mut tx = begin_as(&app, &actor).await.unwrap();
    for (kind, suppressed) in [
        ("assigned", true),
        ("reminder", false),
        ("calendar_changed", true),
        ("feed_error", false),
        ("later_business", true),
        ("later_operational", false),
    ] {
        let id: Option<Uuid> = sqlx::query_scalar(
            "select porchlight.pl_emit_notification_event($1,$2,$3,'registry',$1)",
        )
        .bind(h.id)
        .bind(h.user)
        .bind(kind)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert_eq!(id.is_none(), suppressed, "{kind}");
    }
    tx.commit().await.unwrap();
    let after: String = sqlx::query_scalar("select pg_get_functiondef('porchlight.pl_emit_notification_event(uuid,uuid,text,text,uuid,jsonb,uuid,uuid,text)'::regprocedure)").fetch_one(&pool).await.unwrap();
    assert_eq!(before, after);
    // Unknown kinds fail the FK, both via the definer and via privileged direct SQL.
    let error = sqlx::query(
        "select porchlight.pl_emit_notification_event($1,$2,'unknown_kind','registry',$1)",
    )
    .bind(h.id)
    .bind(h.user)
    .execute(&app)
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    let error = sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id) values ($1,$2,'unknown_kind','registry',$1)")
        .bind(h.id).bind(h.user).execute(&pool).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
}

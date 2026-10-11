use chrono::{DateTime, Utc};
use porchlight_api::{
    calendar,
    extract::Caller,
    items::ics_import::apply_calendar,
    testing::{TestOptions, TestServer, trust},
};
use porchlight_db::tx::{Actor, begin_as};
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn historical_deleted_mapping_does_not_warn_but_hidden_live_mapping_does(pool: PgPool) {
    let now: DateTime<Utc> = "2026-10-06T12:00:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let (_, other) = trust::user_named(&pool, &s, "sam", "Sam Rivera").await;
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let source: Uuid = sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url) values($1,$2,$2,'feed','fixture','fixture','https://example.invalid/calendar') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&mut *tx).await.unwrap();
    let source = calendar::load_source(&mut tx, s.init.household_id, source, true)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let agent = Caller::Agent {
        token_id: Uuid::nil(),
        member_id: s.init.admin_member_id,
        label: "fixture".into(),
        scopes: vec!["admin".into()],
        household_id: s.init.household_id,
        request_id: Uuid::now_v7(),
    };
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let result = apply_calendar(&mut tx, &agent, &source, "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:fixture\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Fixture\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n", true, now).await.unwrap();
    assert_eq!(result.created, 1);
    tx.commit().await.unwrap();
    sqlx::query("update porchlight.items set owner_id=$2,subject_member_id=null,visibility='private',deleted_at=$3 where ics_source_id=$1")
        .bind(source.id).bind(other).bind(now).execute(&pool).await.unwrap();
    let caller = Caller::Member {
        member_id: s.init.admin_member_id,
        user_id: sqlx::query_scalar("select user_id from porchlight.household_members where id=$1")
            .bind(s.init.admin_member_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        role: "super_admin".into(),
        device_id: Uuid::nil(),
        session_id: Uuid::nil(),
        household_id: s.init.household_id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    };
    for deleted in [true, false] {
        if !deleted {
            sqlx::query("update porchlight.items set deleted_at=null where ics_source_id=$1")
                .bind(source.id)
                .execute(&pool)
                .await
                .unwrap();
        }
        let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
        let result = apply_calendar(
            &mut tx,
            &caller,
            &source,
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
            true,
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(result.deleted, 0);
        assert_eq!(
            result
                .warnings
                .iter()
                .filter(|w| w.code == "InvisibleItem")
                .count(),
            usize::from(!deleted),
            "only hidden live mappings require a removal warning"
        );
    }
    s.stop().await;
}

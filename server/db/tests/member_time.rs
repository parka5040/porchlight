use porchlight_db::{
    members,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn authority_claim_reports_manual_and_explicit_transition(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut devices = Vec::new();
    for label in ["One", "Two"] {
        let id:Uuid=sqlx::query_scalar("insert into porchlight.devices(household_id,member_id,label,platform) values($1,$2,$3,'desktop') returning id").bind(h.id).bind(h.user).bind(label).fetch_one(&pool).await.unwrap();
        devices.push(id);
    }
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let actor = Actor {
        member_id: Some(h.user),
        device_id: Some(devices[0]),
        ..Actor::system()
    };
    assert!(
        members::report_device_zone(&app, &actor, h.user, devices[0], chrono_tz::Europe::London)
            .await
            .unwrap()
    );
    let mut c = app.acquire().await.unwrap();
    let one = members::get(&mut c, h.user).await.unwrap();
    assert_eq!(one.tzid, "Europe/London");
    assert_eq!(one.tz_authority_device_id, Some(devices[0]));
    drop(c);
    assert!(
        !members::report_device_zone(&app, &actor, h.user, devices[1], chrono_tz::Asia::Tokyo)
            .await
            .unwrap()
    );
    assert!(
        members::report_device_zone(&app, &actor, h.user, devices[0], chrono_tz::Asia::Tokyo)
            .await
            .unwrap()
    );
    let mut tx = begin_as(&app, &actor).await.unwrap();
    let current = members::get(&mut tx, h.user).await.unwrap();
    let manual = members::set_time_zone(
        &mut tx,
        h.id,
        h.user,
        current.version,
        chrono_tz::UTC,
        "2026-10-05T12:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(manual.tz_source, "manual");
    tx.commit().await.unwrap();
    assert!(
        !members::report_device_zone(&app, &actor, h.user, devices[0], chrono_tz::Asia::Tokyo)
            .await
            .unwrap()
    );
    let mut tx = begin_as(&app, &actor).await.unwrap();
    let authority = members::make_authority(
        &mut tx,
        h.id,
        h.user,
        manual.version,
        devices[1],
        "2026-10-05T12:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(authority.tz_source, "device");
    tx.commit().await.unwrap();
    assert!(
        members::report_device_zone(&app, &actor, h.user, devices[1], chrono_tz::Europe::Paris)
            .await
            .unwrap()
    );
    let mut tx = begin_as(&app, &actor).await.unwrap();
    let dependent = members::get(&mut tx, h.dependent).await.unwrap();
    assert_eq!(dependent.tzid, "America/New_York");
    assert!(
        members::set_time_zone(
            &mut tx,
            h.id,
            h.dependent,
            dependent.version,
            chrono_tz::Europe::Paris,
            "2026-10-05T12:00:00Z".parse().unwrap()
        )
        .await
        .is_err()
    );
    tx.rollback().await.unwrap();
    let events: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.outbox where kind='member.changed' and entity_id=$1",
    )
    .bind(h.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    // One S5 member INSERT tickle plus the five successful zone transitions.
    assert_eq!(events, 6);
}

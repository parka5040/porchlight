use porchlight_db::{
    members,
    testing::{Role, pool_as, seed_household},
};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn canonical_effective_zone_is_inline_and_tracks_authorities(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut conn = app.acquire().await.unwrap();
    let zone: String = sqlx::query_scalar(
        "select porchlight.pl_effective_member_zone('dependent','Asia/Tokyo','Europe/Paris')",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("canonical zone SQL function must exist");
    assert_eq!(zone, "Europe/Paris");
    for role in ["user", "super_admin"] {
        let zone: String = sqlx::query_scalar(
            "select porchlight.pl_effective_member_zone($1,'Asia/Tokyo','Europe/Paris')",
        )
        .bind(role)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(zone, "Asia/Tokyo");
    }
    let plan: Vec<String> = sqlx::query_scalar("explain (verbose, costs off) select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id")
        .fetch_all(&mut *conn).await.unwrap();
    assert!(
        !plan.join("\n").contains("pl_effective_member_zone"),
        "simple SQL expression should inline"
    );
    sqlx::query("update porchlight.households set tzid='Europe/Paris' where id=$1")
        .bind(h.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        members::get(&mut conn, h.dependent).await.unwrap().tzid,
        "Europe/Paris"
    );
    sqlx::query("update porchlight.household_members set tzid='Asia/Tokyo' where id=$1")
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        members::get(&mut conn, h.user).await.unwrap().tzid,
        "Asia/Tokyo"
    );
    assert_eq!(
        members::get(&mut conn, h.dependent).await.unwrap().tzid,
        "Europe/Paris"
    );
}

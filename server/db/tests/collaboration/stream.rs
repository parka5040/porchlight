use porchlight_db::{
    outbox::assign_stream_seqs,
    stream,
    testing::{Role, pool_as, seed_household},
};
use sqlx::PgPool;
use uuid::Uuid;

async fn project(pool: &PgPool) {
    while assign_stream_seqs(pool, 500).await.unwrap().is_some() {}
}
async fn item(pool: &PgPool, h: &porchlight_db::testing::Household, visibility: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,visibility) VALUES ($1,$2,'Private notes never leave DB','todo',$3) RETURNING id")
        .bind(h.id).bind(h.super_admin).bind(visibility).fetch_one(pool).await.unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn authoritative_allow_list_visibility_and_relationship_eviction(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let private = item(&pool, &h, "private").await;
    let assigned = item(&pool, &h, "private").await;
    let selected = item(&pool, &h, "selected").await;
    let private_viewer = item(&pool, &h, "private").await;
    sqlx::query(
        "INSERT INTO porchlight.item_assignees(household_id,item_id,member_id) VALUES ($1,$2,$3)",
    )
    .bind(h.id)
    .bind(assigned)
    .bind(h.user)
    .execute(&pool)
    .await
    .unwrap();
    for id in [selected, private_viewer] {
        sqlx::query(
            "INSERT INTO porchlight.item_viewers(household_id,item_id,member_id) VALUES ($1,$2,$3)",
        )
        .bind(h.id)
        .bind(id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    }
    let occurrence:Uuid=sqlx::query_scalar("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq) VALUES ($1,$2,0) RETURNING id").bind(h.id).bind(selected).fetch_one(&pool).await.unwrap();
    sqlx::query("UPDATE porchlight.item_occurrences SET board_position='a180' WHERE id=$1")
        .bind(occurrence)
        .execute(&pool)
        .await
        .unwrap();
    let secret_occurrence:Uuid=sqlx::query_scalar("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq) VALUES ($1,$2,0) RETURNING id").bind(h.id).bind(private).fetch_one(&pool).await.unwrap();
    sqlx::query("UPDATE porchlight.item_occurrences SET board_position='a180' WHERE id=$1")
        .bind(secret_occurrence)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE porchlight.household_members SET tzid='Europe/London' WHERE id=$1")
        .bind(h.super_admin)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id) VALUES ($1,'derived','item',$2),($1,'item.changed','unknown',$2),($1,'unknown.future.kind','item',$2)").bind(h.id).bind(selected).execute(&pool).await.unwrap();
    // Derived agenda presence must not grant access to a private item.
    sqlx::query("INSERT INTO porchlight.member_agenda(household_id,member_id,occurrence_id,item_id) VALUES ($1,$2,$3,$4)").bind(h.id).bind(h.user).bind(secret_occurrence).bind(private).execute(&pool).await.unwrap();
    project(&pool).await;
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let batch = stream::read(&app, h.id, h.user, 0).await.unwrap();
    let ids: Vec<_> = batch.events.iter().map(|e| e.entity_id).collect();
    assert!(ids.contains(&assigned));
    assert!(ids.contains(&selected));
    assert!(ids.contains(&occurrence));
    assert!(ids.contains(&h.super_admin));
    assert!(!ids.contains(&private));
    assert!(!ids.contains(&private_viewer));
    assert!(!ids.contains(&secret_occurrence));
    assert!(
        batch
            .events
            .iter()
            .any(|e| e.kind == "derived" && e.entity_id == selected)
    );
    assert!(
        batch
            .events
            .iter()
            .all(|e| e.entity_type != "unknown" && e.kind != "unknown.future.kind")
    );
    assert_eq!(batch.cursor, stream::maximum(&app).await.unwrap());
    assert!(batch.events.windows(2).all(|w| w[0].seq < w[1].seq));
    let owner = stream::read(&app, h.id, h.super_admin, 0).await.unwrap();
    assert!(owner.events.iter().any(|e| e.kind == "derived"));
    let dependent = stream::read(&app, h.id, h.dependent, 0).await.unwrap();
    assert!(dependent.events.iter().all(|e| e.kind != "derived"));
    let cursor = batch.cursor;
    sqlx::query("DELETE FROM porchlight.item_assignees WHERE item_id=$1")
        .bind(assigned)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM porchlight.item_viewers WHERE item_id=$1")
        .bind(selected)
        .execute(&pool)
        .await
        .unwrap();
    project(&pool).await;
    let batch = stream::read(&app, h.id, h.user, cursor).await.unwrap();
    assert_eq!(batch.events.len(), 2);
    assert!(batch.events.iter().all(|e| e.kind == "item.access_changed"));
    assert!(batch.events.iter().any(|e| e.entity_id == assigned));
    assert!(batch.events.iter().any(|e| e.entity_id == selected));
    app.close().await;
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn fully_filtered_batch_advances_watermark(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    project(&pool).await;
    let initial = stream::maximum(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id) SELECT $1,'derived','item',$2 FROM generate_series(1,501)").bind(h.id).bind(Uuid::now_v7()).execute(&pool).await.unwrap();
    project(&pool).await;
    let batch = stream::read(&pool, h.id, h.user, initial).await.unwrap();
    assert_eq!(batch.scanned, 500);
    assert!(batch.events.is_empty());
    assert!(batch.cursor > 0);
    let rest = stream::read(&pool, h.id, h.user, batch.cursor)
        .await
        .unwrap();
    assert_eq!(rest.scanned, 1);
    assert!(rest.events.is_empty());
    assert!(rest.cursor > batch.cursor);
    let empty = stream::read(&pool, h.id, h.user, rest.cursor)
        .await
        .unwrap();
    assert_eq!(empty.scanned, 0);
    assert_eq!(empty.cursor, rest.cursor);
}

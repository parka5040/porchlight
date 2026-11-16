use porchlight_db::{
    outbox::assign_stream_seqs,
    stream,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
async fn project(pool: &PgPool) {
    while assign_stream_seqs(pool, 500).await.unwrap().is_some() {}
}
async fn item(pool: &PgPool, h: Uuid, owner: Uuid, visibility: &str) -> Uuid {
    sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'Fixture','todo',$3) returning id")
        .bind(h).bind(owner).bind(visibility).fetch_one(pool).await.unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn parent_hints_ignore_forged_payloads_and_recheck_queued_acl(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let visible = item(&pool, h.id, h.super_admin, "household").await;
    let hidden = item(&pool, h.id, h.super_admin, "private").await;
    let mut comments = Vec::new();
    for key in [visible, hidden] {
        let c: Uuid=sqlx::query_scalar("insert into porchlight.comments(household_id,item_id,author_id,body) values($1,$2,$3,'Fixture') returning id")
            .bind(h.id).bind(key).bind(h.super_admin).fetch_one(&pool).await.unwrap();
        comments.push(c);
    }
    let list: Uuid=sqlx::query_scalar("insert into porchlight.lists(household_id,item_id,owner_id,name) values($1,$2,$3,'Attached') returning id")
        .bind(h.id).bind(visible).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    let entry: Uuid=sqlx::query_scalar("insert into porchlight.list_entries(household_id,list_id,label) values($1,$2,'Entry') returning id")
        .bind(h.id).bind(list).fetch_one(&pool).await.unwrap();
    project(&pool).await;
    let cursor = stream::maximum(&pool).await.unwrap();
    for (comment, spoof) in [(comments[0], hidden), (comments[1], visible)] {
        sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id,payload) values($1,'comment.changed','comment',$2,$3)")
            .bind(h.id).bind(comment).bind(json!({"item_id":spoof})).execute(&pool).await.unwrap();
    }
    sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id,payload) values($1,'list_entry.changed','list_entry',$2,$3)")
        .bind(h.id).bind(entry).bind(json!({"item_id":hidden,"list_id":Uuid::now_v7()})).execute(&pool).await.unwrap();
    project(&pool).await;
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let events = stream::read_capable(&app, h.id, h.user, cursor, true)
        .await
        .unwrap()
        .events;
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| e.item_id == Some(visible)));
    assert_eq!(
        events
            .iter()
            .find(|e| e.entity_id == entry)
            .unwrap()
            .list_id,
        Some(list)
    );
    assert!(events.iter().all(|e| e.entity_id != comments[1]));
    let seqs: Vec<_> = events.iter().map(|e| e.seq).collect();
    sqlx::query("update porchlight.items set visibility='private' where id=$1")
        .bind(visible)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        stream::delivery_batch_capable(&app, h.id, h.user, &seqs, true)
            .await
            .unwrap()
            .events
            .is_empty()
    );
    app.close().await;
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn invalidations_are_transaction_deduped_capped_and_savepoint_safe(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut tx = begin_as(&pool, &Actor::system()).await.unwrap();
    sqlx::query("insert into porchlight.items(household_id,owner_id,title,kind,visibility) select $1,$2,'Bounded peer','todo','household' from generate_series(1,230)")
        .bind(h.id).bind(h.super_admin).execute(&mut *tx).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq) select household_id,id,0 from porchlight.items where household_id=$1")
        .bind(h.id).execute(&mut *tx).await.unwrap();
    sqlx::query("SAVEPOINT probe")
        .execute(&mut *tx)
        .await
        .unwrap();
    let agenda = "insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,busy) select i.household_id,$2,i.id,o.id,tstzrange('2027-03-14T06:30Z','2027-03-14T07:30Z','[)') from porchlight.items i join porchlight.item_occurrences o on o.item_id=i.id where i.household_id=$1";
    sqlx::query(agenda)
        .bind(h.id)
        .bind(h.super_admin)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("ROLLBACK TO SAVEPOINT probe")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(agenda)
        .bind(h.id)
        .bind(h.super_admin)
        .execute(&mut *tx)
        .await
        .unwrap();
    for _ in 0..3 {
        sqlx::query("update porchlight.items set title='Changed title' where household_id=$1")
            .bind(h.id)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    let counts: (i64,i64)=sqlx::query_as("select count(*) filter(where kind='item.invalidated'),count(*) filter(where kind='sync.reset') from porchlight.outbox where household_id=$1")
        .bind(h.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(counts, (200, 1));
    tx.commit().await.unwrap();
    project(&pool).await;
    let mut cursor = 0;
    let mut legacy = Vec::new();
    let mut capable = Vec::new();
    loop {
        let b = stream::read(&pool, h.id, h.user, cursor).await.unwrap();
        capable.extend(
            stream::read_capable(&pool, h.id, h.user, cursor, true)
                .await
                .unwrap()
                .events,
        );
        legacy.extend(b.events);
        cursor = b.cursor;
        if b.scanned < stream::BATCH {
            break;
        }
    }
    assert!(
        legacy
            .iter()
            .all(|e| e.kind != "item.invalidated" && e.kind != "sync.reset")
    );
    assert_eq!(capable.iter().filter(|e| e.kind == "sync.reset").count(), 1);
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn notification_acl_source_severing_and_delete_are_recipient_scoped(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let key = item(&pool, h.id, h.super_admin, "household").await;
    let event: Uuid=sqlx::query_scalar("select porchlight.pl_emit_notification_event($1,$2,'comment','item',$3,'{}',$3,null,'source')")
        .bind(h.id).bind(h.user).bind(key).fetch_one(&pool).await.unwrap();
    let notification: Uuid=sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,entity_type,entity_id,window_start,release_at,released_at) values($1,$2,'comment','activity','item',$3,now(),now(),now()) returning id")
        .bind(h.id).bind(h.user).bind(key).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)",
    )
    .bind(notification)
    .bind(event)
    .execute(&pool)
    .await
    .unwrap();
    for query in [
        "update porchlight.items set visibility='private' where id=$1",
        "delete from porchlight.notification_sources where event_id=$1",
        "delete from porchlight.notifications where id=$1",
    ] {
        project(&pool).await;
        let cursor = stream::maximum(&pool).await.unwrap();
        let key = if query.contains("sources") {
            event
        } else if query.contains("notifications where") {
            notification
        } else {
            key
        };
        sqlx::query(query).bind(key).execute(&pool).await.unwrap();
        project(&pool).await;
        let recipient = stream::read_capable(&pool, h.id, h.user, cursor, true)
            .await
            .unwrap();
        assert!(
            recipient
                .events
                .iter()
                .any(|e| e.kind == "notification.invalidated" && e.entity_id == notification),
            "{query}"
        );
        let other = stream::read_capable(&pool, h.id, h.super_admin, cursor, true)
            .await
            .unwrap();
        assert!(
            other
                .events
                .iter()
                .all(|e| e.kind != "notification.invalidated")
        );
        assert!(
            stream::read(&pool, h.id, h.user, cursor)
                .await
                .unwrap()
                .events
                .iter()
                .all(|e| e.kind != "notification.invalidated")
        );
    }
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn historical_and_current_availability_are_negotiated_and_private(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    project(&pool).await;
    let cursor = stream::maximum(&pool).await.unwrap();
    sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id,payload) values($1,'member.changed','availability',$2,jsonb_build_object('member_id',$2::text))")
        .bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00')")
        .bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    project(&pool).await;
    let events = stream::read_capable(&pool, h.id, h.user, cursor, true)
        .await
        .unwrap()
        .events;
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|e| e.kind == "settings.changed" && e.entity_type == "availability")
    );
    assert!(
        stream::read(&pool, h.id, h.user, cursor)
            .await
            .unwrap()
            .events
            .is_empty()
    );
    assert!(
        stream::read_capable(&pool, h.id, h.super_admin, cursor, true)
            .await
            .unwrap()
            .events
            .is_empty()
    );
}

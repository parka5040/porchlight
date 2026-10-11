use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn counted_collaboration_and_notification_purge(pool: PgPool) {
    let h: Uuid = sqlx::query_scalar(
        "insert into porchlight.households(name,tzid) values('O2','UTC') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let m: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Dependent','dependent','#123456','UTC') returning id").bind(h).fetch_one(&pool).await.unwrap();
    let i: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,title,kind,owner_id,deleted_at) values($1,'purge','todo',$2,now()) returning id").bind(h).bind(m).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.comments(household_id,item_id,author_id,body) values($1,$2,$3,'comment')").bind(h).bind(i).bind(m).execute(&pool).await.unwrap();
    let e: Uuid = sqlx::query_scalar("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id) values($1,$2,'assigned',$3,'item',$3) returning id").bind(h).bind(m).bind(i).fetch_one(&pool).await.unwrap();
    let n: Uuid = sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,item_id,entity_type,entity_id,window_start,release_at) values($1,$2,'assigned','assignments',$3,'item',$3,now(),now()) returning id").bind(h).bind(m).bind(i).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.notification_sources values($1,$2)")
        .bind(n)
        .bind(e)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.notification_event_delivery(event_id,attempts,retry_at,last_error) values($1,1,now(),'constraint')").bind(e).execute(&pool).await.unwrap();
    for hash in ["a".repeat(64), "b".repeat(64)] {
        sqlx::query("insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,$2,1,'image/png')").bind(h).bind(hash).execute(&pool).await.unwrap();
    }
    sqlx::query("insert into porchlight.attachments(household_id,item_id,uploader_id,original_name,blob_sha256,thumbnail_sha256) values($1,$2,$3,'fixture.png',$4,$5)").bind(h).bind(i).bind(m).bind("a".repeat(64)).bind("b".repeat(64)).execute(&pool).await.unwrap();
    sqlx::query(
        "update porchlight.blobs set last_referenced_at='2000-01-01T00:00Z' where household_id=$1",
    )
    .bind(h)
    .execute(&pool)
    .await
    .unwrap();
    for (parent, columns, refs, selection) in [
        (
            "comments",
            "parent uuid",
            "id",
            "select id from porchlight.comments where item_id=$1",
        ),
        (
            "attachments",
            "parent uuid",
            "id",
            "select id from porchlight.attachments where item_id=$1",
        ),
        (
            "notification_sources",
            "parent uuid, event uuid",
            "notification_id,event_id",
            "select notification_id,event_id from porchlight.notification_sources where event_id=$1",
        ),
        (
            "notification_event_delivery",
            "parent uuid",
            "event_id",
            "select event_id from porchlight.notification_event_delivery where event_id=$1",
        ),
    ] {
        let fk_cols = if parent == "notification_sources" {
            "parent,event"
        } else {
            "parent"
        };
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("create table porchlight.o2_unknown({columns}, foreign key({fk_cols}) references porchlight.{parent}({refs}) on delete cascade); alter table porchlight.o2_unknown owner to porchlight_owner"))).execute(&pool).await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "insert into porchlight.o2_unknown {selection}"
        )))
        .bind(if parent.starts_with("notification") {
            e
        } else {
            i
        })
        .execute(&pool)
        .await
        .unwrap();
        let err = sqlx::query("select porchlight.pl_purge_items($1,$2)")
            .bind(h)
            .bind(vec![i])
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("PL409"),
            "{parent}"
        );
        let present: i64 = sqlx::query_scalar("select count(*) from porchlight.items where id=$1")
            .bind(i)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(present, 1);
        sqlx::query("drop table porchlight.o2_unknown")
            .execute(&pool)
            .await
            .unwrap();
    }
    let result: serde_json::Value = sqlx::query_scalar("select porchlight.pl_purge_items($1,$2)")
        .bind(h)
        .bind(vec![i])
        .fetch_one(&pool)
        .await
        .unwrap();
    for key in [
        "items",
        "comments",
        "attachments",
        "notification_events",
        "notification_sources",
        "notification_event_delivery",
        "notifications",
    ] {
        assert_eq!(result["counts"][key], 1, "{key}: {result}");
    }
    assert_eq!(result["counts"]["blob_references"], 2);
    let reset: i64=sqlx::query_scalar("select count(*) from porchlight.blobs where household_id=$1 and last_referenced_at>'2000-01-01T00:00Z'").bind(h).fetch_one(&pool).await.unwrap();
    assert_eq!(
        reset, 2,
        "main/thumbnail metadata survive with reset GC clocks"
    );
    let item: Option<Uuid> =
        sqlx::query_scalar("select item_id from porchlight.notifications where id=$1")
            .bind(n)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        item, None,
        "retain recipient history, explicitly sever purged reference"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn occurrence_only_notification_is_removed_and_counted(pool: PgPool) {
    use porchlight_db::testing::{Role, insert_appointment, pool_as, seed_household};
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let occurrence: Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0) returning id")
        .bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let event: Uuid = sqlx::query_scalar("select porchlight.pl_emit_notification_event($1,$2,'assigned','occurrence',$3,'{}',NULL,$3,NULL)")
        .bind(h.id).bind(h.user).bind(occurrence).fetch_one(&app).await.unwrap();
    sqlx::query("insert into porchlight.notification_event_delivery(event_id,attempts,retry_at,last_error) values($1,1,now(),'constraint')")
        .bind(event).execute(&pool).await.unwrap();
    let notification: Uuid = sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,occurrence_id,entity_type,entity_id,window_start,release_at) values($1,$2,'assigned','assignments',$3,'occurrence',$3,now(),now()) returning id")
        .bind(h.id).bind(h.user).bind(occurrence).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.notification_sources values($1,$2)")
        .bind(notification)
        .bind(event)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    let result: serde_json::Value = sqlx::query_scalar("select porchlight.pl_purge_items($1,$2)")
        .bind(h.id)
        .bind(vec![item])
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(result["counts"]["notification_events"], 1);
    let remaining: i64 =
        sqlx::query_scalar("select count(*) from porchlight.notification_events where id=$1")
            .bind(event)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 0,
        "purge counted an occurrence-only event but left it orphaned"
    );
    for table in ["notification_event_delivery", "notification_sources"] {
        let remaining: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select count(*) from porchlight.{table} where event_id=$1"
        )))
        .bind(event)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0);
        assert_eq!(result["counts"][table], 1);
    }
    let audit: serde_json::Value =
        sqlx::query_scalar("select counts from porchlight.purge_log where id=$1")
            .bind(Uuid::parse_str(result["id"].as_str().unwrap()).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(audit, result["counts"]);
    let retained: bool = sqlx::query_scalar("select item_id is null and occurrence_id is null from porchlight.notifications where id=$1")
        .bind(notification).fetch_one(&pool).await.unwrap();
    assert!(
        retained,
        "recipient history retained with references severed"
    );
}

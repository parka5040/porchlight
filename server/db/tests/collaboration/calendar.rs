use porchlight_db::testing::{Role, connect_as, seed_household};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn source_configuration_is_attributed_versioned_and_tickles_only_admins(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:calendar-test"))
        .await
        .unwrap();
    let id: uuid::Uuid = sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url) values($1,$2,$3,'feed','school','school','https://example.invalid/calendar') returning id")
        .bind(h.id).bind(h.super_admin).bind(h.user).fetch_one(&mut agent).await.unwrap();
    sqlx::query("update porchlight.ics_sources set name='renamed' where id=$1")
        .bind(id)
        .execute(&mut agent)
        .await
        .unwrap();
    let revisions: Vec<(i32, serde_json::Value)> = sqlx::query_as("select source_version,actor from porchlight.ics_source_revisions where source_id=$1 order by source_version").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[1].0, 2);
    assert_eq!(revisions[1].1["application_name"], "agent:calendar-test");
    let payloads: Vec<serde_json::Value> = sqlx::query_scalar(
        "select payload from porchlight.outbox where entity_id=$1 and kind='ics_source.changed'",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(payloads.len(), 2);
    for payload in payloads {
        assert_eq!(payload["member_id"], h.super_admin.to_string());
        assert_eq!(payload.as_object().unwrap().len(), 2);
    }
    for field in ["next_fetch_at", "last_fetch_at"] {
        let query = format!("update porchlight.ics_sources set {field}=now() where id=$1");
        let e = sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(id)
            .execute(&mut agent)
            .await
            .unwrap_err();
        assert_eq!(
            e.as_database_error().unwrap().code().as_deref(),
            Some("42501")
        );
    }
    let e = sqlx::query("insert into porchlight.ics_sources(household_id,owner_member_id,kind,import_key,name,url,last_status) values($1,$2,'feed','fake','fake','https://example.invalid/calendar','ok')")
        .bind(h.id).bind(h.super_admin).execute(&mut agent).await.unwrap_err();
    assert_eq!(
        e.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_source_settings_propagate_and_removal_retires_items(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:calendar-settings"))
        .await
        .unwrap();
    let source:uuid::Uuid=sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url) values($1,$2,$2,'feed','raw','raw','https://example.invalid/calendar') returning id").bind(h.id).bind(h.super_admin).fetch_one(&mut agent).await.unwrap();
    sqlx::query("update porchlight.items set ics_source_id=$2,visibility='private' where id=$1")
        .bind(item)
        .bind(source)
        .execute(&mut agent)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.ics_uid_map(household_id,source_id,uid,recurrence_key,item_id) values($1,$2,'raw','',$3)").bind(h.id).bind(source).bind(item).execute(&pool).await.unwrap();
    sqlx::query("update porchlight.ics_sources set visibility='household' where id=$1")
        .bind(source)
        .execute(&mut agent)
        .await
        .unwrap();
    let visibility: String =
        sqlx::query_scalar("select visibility from porchlight.items where id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(visibility, "household");
    sqlx::query("update porchlight.ics_sources set target_member_id=null where id=$1")
        .bind(source)
        .execute(&mut agent)
        .await
        .unwrap();
    let assignees: i64 =
        sqlx::query_scalar("select count(*) from porchlight.item_assignees where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(assignees, 0);
    sqlx::query("update porchlight.ics_sources set target_member_id=$2 where id=$1")
        .bind(source)
        .bind(h.dependent)
        .execute(&mut agent)
        .await
        .unwrap();
    let dependent_row: (uuid::Uuid, Option<uuid::Uuid>, String) = sqlx::query_as(
        "select owner_id,subject_member_id,visibility from porchlight.items where id=$1",
    )
    .bind(item)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        dependent_row,
        (h.super_admin, Some(h.dependent), "household".into())
    );
    for visibility in ["selected", "explicit", "invalid"] {
        let error = sqlx::query("update porchlight.ics_sources set visibility=$2 where id=$1")
            .bind(source)
            .bind(visibility)
            .execute(&mut agent)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("23514")
        );
    }
    let default:String=sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url) values($1,$2,$3,'feed','dependent','dependent','https://example.invalid/calendar') returning visibility").bind(h.id).bind(h.super_admin).bind(h.dependent).fetch_one(&mut agent).await.unwrap();
    assert_eq!(default, "household");
    sqlx::query("update porchlight.ics_sources set deleted_at=now(),enabled=false where id=$1")
        .bind(source)
        .execute(&mut agent)
        .await
        .unwrap();
    let retired:(bool,i64)=sqlx::query_as("select deleted_at is not null,(select count(*) from porchlight.ics_uid_map where source_id=$2) from porchlight.items where id=$1").bind(item).bind(source).fetch_one(&pool).await.unwrap();
    assert_eq!(retired, (true, 0));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn fix3_assignees_survive_non_target_config_and_warnings_are_not_snapshotted(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:calendar-assignees"))
        .await
        .unwrap();
    let source: uuid::Uuid = sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url) values($1,$2,$2,'feed','raw','raw','https://example.invalid/calendar') returning id").bind(h.id).bind(h.super_admin).fetch_one(&mut agent).await.unwrap();
    sqlx::query("update porchlight.items set ics_source_id=$2 where id=$1")
        .bind(item)
        .bind(source)
        .execute(&mut agent)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3) on conflict do nothing").bind(h.id).bind(item).bind(h.user).execute(&mut agent).await.unwrap();
    for query in [
        "update porchlight.ics_sources set name='rename' where id=$1",
        "update porchlight.ics_sources set visibility='household' where id=$1",
    ] {
        sqlx::query(query)
            .bind(source)
            .execute(&mut agent)
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.item_assignees where item_id=$1 and member_id=$2",
        )
        .bind(item)
        .bind(h.user)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            count, 1,
            "rename/visibility must preserve agent-added assignments"
        );
    }
    sqlx::query("update porchlight.ics_sources set target_member_id=$2 where id=$1")
        .bind(source)
        .bind(h.dependent)
        .execute(&mut agent)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("select count(*) from porchlight.item_assignees where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "a target change resets assignments for the new agenda"
    );
    sqlx::query("update porchlight.ics_sources set last_warnings='[{\"code\":\"InvalidEvent\",\"uid\":\"remote\",\"message\":\"remote text\"}]' where id=$1").bind(source).execute(&pool).await.unwrap();
    let leaked: bool = sqlx::query_scalar("select exists(select 1 from porchlight.ics_source_revisions where source_id=$1 and snapshot ? 'last_warnings')").bind(source).fetch_one(&pool).await.unwrap();
    assert!(
        !leaked,
        "operational remote warnings must not enter append-only revision history"
    );
}

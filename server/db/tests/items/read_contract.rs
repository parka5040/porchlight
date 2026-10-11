use porchlight_db::{
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn notification_contract_actor_dedupe_self_and_grants(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let actor = Actor {
        member_id: Some(h.user),
        ..Actor::system()
    };
    let mut tx = begin_as(&app, &actor).await.unwrap();
    let emit = "select porchlight.pl_emit_notification_event($1,$2,$3,'item',$4,'{}',null,null,$5)";
    let own: Option<uuid::Uuid> = sqlx::query_scalar(emit)
        .bind(h.id)
        .bind(h.user)
        .bind("assigned")
        .bind(h.id)
        .bind("self")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(own.is_none());
    let first: Option<uuid::Uuid> = sqlx::query_scalar(emit)
        .bind(h.id)
        .bind(h.super_admin)
        .bind("assigned")
        .bind(h.id)
        .bind("dedupe")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(first.is_some());
    let duplicate: Option<uuid::Uuid> = sqlx::query_scalar(emit)
        .bind(h.id)
        .bind(h.super_admin)
        .bind("assigned")
        .bind(h.id)
        .bind("dedupe")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(duplicate.is_none());
    let reminder: Option<uuid::Uuid> = sqlx::query_scalar(emit)
        .bind(h.id)
        .bind(h.user)
        .bind("reminder")
        .bind(h.id)
        .bind("reminder")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(reminder.is_some());
    tx.commit().await.unwrap();
    let stored: Option<uuid::Uuid> = sqlx::query_scalar(
        "select actor_member_id from porchlight.notification_events where id=$1",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, Some(h.user));
    let agent = pool_as(&pool, Role::Agent, Some("agent:contract"))
        .await
        .unwrap();
    let id: Option<uuid::Uuid> = sqlx::query_scalar(emit)
        .bind(h.id)
        .bind(h.user)
        .bind("comment")
        .bind(h.id)
        .bind("agent")
        .fetch_one(&agent)
        .await
        .unwrap();
    let (member, label): (Option<uuid::Uuid>, Option<String>) = sqlx::query_as(
        "select actor_member_id,actor_label from porchlight.notification_events where id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(member, None);
    assert_eq!(label.as_deref(), Some("agent:contract"));
    let denied = sqlx::query("update porchlight.notification_events set payload='{}' where id=$1")
        .bind(id)
        .execute(&app)
        .await
        .unwrap_err();
    assert_eq!(
        denied.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    assert!(sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id) values($1,$2,'comment','item',$1)").bind(h.id).bind(h.user).execute(&agent).await.is_err());
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn settings_events_name_private_scope_and_search_acl_is_authoritative(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("update porchlight.member_settings set settings='{}' where household_id=$1 and member_id=$2").bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    let scope:String=sqlx::query_scalar("select payload->>'member_id' from porchlight.outbox where kind='settings.changed' and entity_id=$1 order by id desc limit 1").bind(h.user).fetch_one(&pool).await.unwrap();
    assert_eq!(scope, h.user.to_string());
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'Café plumbing','todo','private') returning id").bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    let can: bool = sqlx::query_scalar("select porchlight.pl_item_can_read($1,$2)")
        .bind(id)
        .bind(h.super_admin)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!can);
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(h.id)
    .bind(id)
    .bind(h.super_admin)
    .execute(&pool)
    .await
    .unwrap();
    let can: bool = sqlx::query_scalar("select porchlight.pl_item_can_read($1,$2)")
        .bind(id)
        .bind(h.super_admin)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(can);
    let found:bool=sqlx::query_scalar("select search_document @@ plainto_tsquery('simple','plumbing') from porchlight.items where id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(found);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn notification_api_agent_label_and_raw_actor_spoof_protection(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let actor = Actor {
        member_id: Some(h.user),
        agent_label: Some("producer".into()),
        token_id: Some(uuid::Uuid::now_v7()),
        ..Actor::system()
    };
    let mut tx = begin_as(&app, &actor).await.unwrap();
    let emit = "select porchlight.pl_emit_notification_event($1,$2,'comment','item',$1)";
    let id: Option<uuid::Uuid> = sqlx::query_scalar(emit)
        .bind(h.id)
        .bind(h.super_admin)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let captured: (Option<uuid::Uuid>, Option<String>) = sqlx::query_as(
        "select actor_member_id,actor_label from porchlight.notification_events where id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(captured, (Some(h.user), Some("agent:producer".into())));
    let agent = pool_as(&pool, Role::Agent, Some("agent:raw-producer"))
        .await
        .unwrap();
    let mut tx = agent.begin().await.unwrap();
    sqlx::query("select set_config('porchlight.actor_member_id',$1,true)")
        .bind(h.user.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let id: Option<uuid::Uuid> = sqlx::query_scalar(emit)
        .bind(h.id)
        .bind(h.user)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(
        id.is_some(),
        "spoofed member must not suppress raw agent event"
    );
    tx.commit().await.unwrap();
    let member: Option<uuid::Uuid> = sqlx::query_scalar(
        "select actor_member_id from porchlight.notification_events where id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(member.is_none());
    let mut tx = agent.begin().await.unwrap();
    sqlx::query("select set_config('application_name','',true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let error = sqlx::query_scalar::<_, Option<uuid::Uuid>>(emit)
        .bind(h.id)
        .bind(h.user)
        .fetch_one(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("PL403")
    );
    tx.rollback().await.unwrap();
    let id = id.unwrap();
    sqlx::query("update porchlight.notification_events set processed_at=now() where id=$1")
        .bind(id)
        .execute(&app)
        .await
        .unwrap();
    assert!(
        sqlx::query("delete from porchlight.notification_events where id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err(),
        "even privileged deletion is forbidden by append-only contract"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn notification_survives_regeneration_and_maintenance_purge(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = porchlight_db::testing::insert_appointment(&pool, &h)
        .await
        .unwrap();
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    let mut tx = pool.begin().await.unwrap();
    porchlight_db::derive::derive_item(&mut tx, item, now)
        .await
        .unwrap();
    let occurrence: uuid::Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) select household_id,id,99 from porchlight.items where id=$1 returning id").bind(item).fetch_one(&mut *tx).await.unwrap();
    sqlx::query(
        "select porchlight.pl_emit_notification_event($1,$2,'reminder','item',$3,'{}',$3,$4)",
    )
    .bind(h.id)
    .bind(h.user)
    .bind(item)
    .bind(occurrence)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("update porchlight.items set starts_at=starts_at+interval '1 day',ends_at=ends_at+interval '1 day' where id=$1").bind(item).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    porchlight_db::derive::derive_item(&mut tx, item, now)
        .await
        .expect("event must not block deleting open occurrence");
    tx.commit().await.unwrap();
    let old: bool =
        sqlx::query_scalar("select exists(select 1 from porchlight.item_occurrences where id=$1)")
            .bind(occurrence)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!old);
    for role in [Role::App, Role::Agent] {
        let login = pool_as(&pool, role, Some("agent:purge-test"))
            .await
            .unwrap();
        assert!(
            sqlx::query("delete from porchlight.notification_events where item_id=$1")
                .bind(item)
                .execute(&login)
                .await
                .is_err()
        );
    }
    // S5 owns maintenance grants; emulate its purge privileges in this fixture.
    sqlx::raw_sql("grant usage on schema porchlight to porchlight_maintenance; grant select,delete on all tables in schema porchlight to porchlight_maintenance").execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("set local role porchlight_maintenance")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("delete from porchlight.notification_events where item_id=$1")
        .bind(item)
        .execute(&mut *tx)
        .await
        .expect("maintenance may delete events first");
    // Remove generated children before audit rows/parent, as S5's existing contract requires.
    for table in [
        "member_agenda",
        "scheduled_reminders",
        "item_occurrences",
        "item_derivation",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "delete from porchlight.{table} where item_id=$1"
        )))
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    // S5 purge also removes the existing append-only audit rows before the item.
    sqlx::query("delete from porchlight.task_events where item_id=$1")
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("delete from porchlight.item_revisions where item_id=$1")
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("delete from porchlight.items where id=$1")
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn notification_processed_at_is_one_way_even_for_privileged_writers(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id: uuid::Uuid = sqlx::query_scalar(
        "select porchlight.pl_emit_notification_event($1,$2,'reminder','item',$1)",
    )
    .bind(h.id)
    .bind(h.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        sqlx::query("update porchlight.notification_events set kind='comment' where id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("update porchlight.notification_events set processed_at=now() where id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        sqlx::query("update porchlight.notification_events set processed_at=null where id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("update porchlight.notification_events set processed_at=now()+interval '1 second' where id=$1").bind(id).execute(&pool).await.is_err());
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn timezone_validators_agree_on_canonical_storage_names(pool: PgPool) {
    // Exhaustive agreement for bundled zone names, including deprecated region links.
    let catalog: Vec<String> = sqlx::query_scalar(
        "select name from pg_timezone_names where name like '%/%' and name !~ '^(posix|right)/'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for zone in chrono_tz::TZ_VARIANTS {
        let name = zone.name();
        let parsed = porchlight_core::time::storage_zone(name)
            .ok()
            .map(|(zone, _)| zone);
        let canonical = parsed.map(|z| z.name()).unwrap_or(name);
        let sql: bool = sqlx::query_scalar("select porchlight.pl_valid_tzid($1)")
            .bind(canonical)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(parsed.is_some(), sql, "{name}");
    }
    for name in catalog {
        assert!(
            porchlight_core::time::parse_tz(&name).is_some(),
            "SQL-only region {name}"
        );
    }
    for name in [
        "UTC",
        "Etc/UTC",
        "GMT",
        "Etc/GMT",
        "Z",
        "Zulu",
        "Universal",
        "America/New_York",
        "Europe/Berlin",
        "Asia/Tokyo",
        "Etc/GMT+12",
        "CET",
        "EET",
        "WET",
        "MET",
        "EST5EDT",
        "PST8PDT",
        "US/Eastern",
        "posix/Europe/Berlin",
        "right/Europe/Berlin",
        "Mars/Olympus_Mons",
        "",
    ] {
        let rust = porchlight_core::time::storage_zone(name)
            .ok()
            .map(|(zone, _)| zone);
        let sql: bool = sqlx::query_scalar("select porchlight.pl_valid_tzid($1)")
            .bind(rust.map(|z| z.name()).unwrap_or(name))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rust.is_some(), sql, "{name}");
        if ["UTC", "Etc/UTC", "GMT", "Etc/GMT", "Z", "Zulu", "Universal"].contains(&name) {
            assert_eq!(rust.unwrap().name(), "UTC");
        }
        if ["CET", "EET", "WET", "MET", "EST5EDT", "PST8PDT"].contains(&name) {
            assert!(rust.is_some(), "{name}");
        }
    }
    let h = seed_household(&pool).await.unwrap();
    let item: uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'ACL','todo') returning id").bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    let dependent: uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Dependent','dependent','#ABCDEF','UTC') returning id").bind(h.id).fetch_one(&pool).await.unwrap();
    let can: bool = sqlx::query_scalar("select porchlight.pl_item_can_read($1,$2)")
        .bind(item)
        .bind(dependent)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!can);
}

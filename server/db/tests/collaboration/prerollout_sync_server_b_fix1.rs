use porchlight_db::{
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::{Connection, PgPool};
use uuid::Uuid;

async fn item(pool: &PgPool, h: Uuid, owner: Uuid) -> Uuid {
    sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'Fixture','todo','household') returning id")
        .bind(h).bind(owner).fetch_one(pool).await.unwrap()
}
async fn count(pool: &PgPool, key: Uuid, kind: &str) -> i64 {
    sqlx::query_scalar("select count(*) from porchlight.outbox where entity_id=$1 and kind=$2")
        .bind(key)
        .bind(kind)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn ancestor_acl_names_nonoverlapping_descendants(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let head = item(&pool, h.id, h.super_admin).await;
    let tail = item(&pool, h.id, h.super_admin).await;
    sqlx::query("update porchlight.items set series_id=$1,split_from_item_id=$1,split_from_seq=1 where id=$2")
        .bind(head).bind(tail).execute(&pool).await.unwrap();
    for sql in [
        "update porchlight.items set visibility='private' where id=$1",
        "update porchlight.items set visibility='selected' where id=$1",
        "update porchlight.items set deleted_at=now() where id=$1",
        "update porchlight.items set deleted_at=null where id=$1",
    ] {
        let before = count(&pool, tail, "item.invalidated").await;
        sqlx::query(sql).bind(head).execute(&pool).await.unwrap();
        assert!(
            count(&pool, tail, "item.invalidated").await > before,
            "ancestor change left tail unnamed: {sql}"
        );
    }
    sqlx::query(
        "insert into porchlight.item_viewers(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(h.id)
    .bind(head)
    .bind(h.user)
    .execute(&pool)
    .await
    .unwrap();
    let before = count(&pool, tail, "item.invalidated").await;
    sqlx::query("delete from porchlight.item_viewers where item_id=$1")
        .bind(head)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        count(&pool, tail, "item.invalidated").await > before,
        "viewer removal left tail unnamed"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn real_agent_presets_cannot_suppress_or_persist_dedupe(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let head = item(&pool, h.id, h.super_admin).await;
    let peer = item(&pool, h.id, h.super_admin).await;
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq) select household_id,id,0 from porchlight.items where household_id=$1")
        .bind(h.id).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,busy) select i.household_id,$2,i.id,o.id,tstzrange('2027-03-14T06:30Z','2027-03-14T07:30Z','[)') from porchlight.items i join porchlight.item_occurrences o on o.item_id=i.id where i.household_id=$1")
        .bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    let event: Uuid = sqlx::query_scalar(
        "select porchlight.pl_emit_notification_event($1,$2,'comment','item',$3,'{}',$3,null,null)",
    )
    .bind(h.id)
    .bind(h.user)
    .bind(head)
    .fetch_one(&pool)
    .await
    .unwrap();
    let notification: Uuid=sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,entity_type,entity_id,window_start,release_at,released_at) values($1,$2,'comment','activity','item',$3,now(),now(),now()) returning id")
        .bind(h.id).bind(h.user).bind(head).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)",
    )
    .bind(notification)
    .bind(event)
    .execute(&pool)
    .await
    .unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:coverage"))
        .await
        .unwrap();
    let mut connection = agent.acquire().await.unwrap();
    let role: String = sqlx::query_scalar("select session_user::text")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    assert_eq!(role, "porchlight_agent");
    for (local, preset) in [(false, 0), (true, 0), (false, 1), (true, 2), (false, 0)] {
        let forged = match preset {
            0 => serde_json::json!({format!("{}:reset", h.id):true}).to_string(),
            1 => serde_json::json!({format!("{}:item.invalidated:{peer}:",h.id):true,format!("{}:notification.invalidated:{notification}:{}",h.id,h.user):true}).to_string(),
            _ => "not JSON at all".to_owned(),
        };
        if !local {
            sqlx::query("select set_config('porchlight.sync_emitted',$1,false)")
                .bind(&forged)
                .execute(&mut *connection)
                .await
                .unwrap();
        }
        let before = count(&pool, peer, "item.invalidated").await;
        let inbox_before = count(&pool, notification, "notification.invalidated").await;
        let mut tx = connection.begin().await.unwrap();
        if local {
            sqlx::query("select set_config('porchlight.sync_emitted',$1,true)")
                .bind(&forged)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        sqlx::query("update porchlight.items set title=title||'x',visibility=case visibility when 'private' then 'household' else 'private' end where id=$1")
            .bind(head).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            count(&pool, peer, "item.invalidated").await,
            before + 1,
            "agent preset suppressed peer invalidation (local={local})"
        );
        assert_eq!(
            count(&pool, notification, "notification.invalidated").await,
            inbox_before + 1,
            "agent preset suppressed inbox invalidation"
        );
    }
    for sql in [
        "select * from porchlight.sync_emissions",
        "delete from porchlight.sync_emissions",
        "insert into porchlight.sync_emissions(backend_pid,transaction_id,household_id) values(pg_backend_pid(),pg_current_xact_id(),gen_random_uuid())",
        "update porchlight.sync_emissions set emitted='{}'",
        "select porchlight.pl_sync_emit(gen_random_uuid(),'sync.reset','household',gen_random_uuid(),null)",
    ] {
        let error = sqlx::query(sql)
            .execute(&mut *connection)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("42501"),
            "agent forged protected state: {sql}"
        );
    }
    drop(connection);
    agent.close().await;
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn actor_rename_names_each_single_and_coalesced_inbox_id(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut tx = begin_as(
        &pool,
        &Actor {
            member_id: Some(h.super_admin),
            ..Actor::system()
        },
    )
    .await
    .unwrap();
    let event: Uuid = sqlx::query_scalar("select porchlight.pl_emit_notification_event($1,$2,'comment','member',$3,'{}',null,null,null)")
        .bind(h.id).bind(h.user).bind(h.super_admin).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let mut ids = Vec::new();
    for coalesced in [false, true] {
        let n: Uuid = sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,entity_type,entity_id,window_start,release_at,released_at) values($1,$2,'comment','activity','member',$3,now(),now(),now()) returning id")
            .bind(h.id).bind(h.user).bind(h.super_admin).fetch_one(&pool).await.unwrap();
        let e = if coalesced {
            let mut tx = begin_as(
                &pool,
                &Actor {
                    member_id: Some(h.super_admin),
                    ..Actor::system()
                },
            )
            .await
            .unwrap();
            let e: Uuid = sqlx::query_scalar("select porchlight.pl_emit_notification_event($1,$2,'comment','member',$3,'{}',null,null,null)")
                .bind(h.id).bind(h.user).bind(h.super_admin).fetch_one(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
            e
        } else {
            event
        };
        sqlx::query(
            "insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)",
        )
        .bind(n)
        .bind(e)
        .execute(&pool)
        .await
        .unwrap();
        if coalesced {
            let mut tx = begin_as(
                &pool,
                &Actor {
                    member_id: Some(h.super_admin),
                    ..Actor::system()
                },
            )
            .await
            .unwrap();
            let e:Uuid = sqlx::query_scalar("select porchlight.pl_emit_notification_event($1,$2,'comment','member',$3,'{}',null,null,null)")
                .bind(h.id).bind(h.user).bind(h.super_admin).fetch_one(&mut *tx).await.unwrap();
            sqlx::query("insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)").bind(n).bind(e).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
        ids.push(n);
    }
    sqlx::query("update porchlight.household_members set display_name='Sam Rivera' where id=$1")
        .bind(h.super_admin)
        .execute(&pool)
        .await
        .unwrap();
    for n in ids {
        assert_eq!(
            count(&pool, n, "notification.invalidated").await,
            1,
            "actor rename left inbox ID unnamed"
        );
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn enqueue_pending_is_announced_before_worker_completion(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let key = item(&pool, h.id, h.super_admin).await;
    sqlx::query("update porchlight.item_derivation set applied_gen=requested_gen where item_id=$1")
        .bind(key)
        .execute(&pool)
        .await
        .unwrap();
    let before = count(&pool, key, "item.invalidated").await;
    sqlx::query("select porchlight.pl_enqueue_derivation($1,$2)")
        .bind(h.id)
        .bind(key)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(&pool, key, "item.invalidated").await,
        before + 1,
        "pending transition has no announcement"
    );
    sqlx::query("select porchlight.pl_enqueue_derivation($1,$2)")
        .bind(h.id)
        .bind(key)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(&pool, key, "item.invalidated").await,
        before + 1,
        "already pending enqueue is redundant"
    );
}

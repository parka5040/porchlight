use super::*;
use porchlight_db::{
    testing::{Role, pool_as},
    tx::{Actor, begin_as},
};

pub(in super::super) async fn mutate_extra(m: &mut Model, op: &str) {
    match op {
        "raw_title" | "ancestor_private" | "ancestor_household" | "ancestor_selected"
        | "ancestor_view_gain" | "ancestor_view_loss" | "ancestor_delete" | "ancestor_restore" => {
            let agent = pool_as(&m.pool, Role::Agent, Some("agent:coverage"))
                .await
                .unwrap();
            let mut tx = begin_as(&agent, &Actor::system()).await.unwrap();
            let key = m.items[0];
            // Real login, not SET ROLE or a PAT request. Preset the historical
            // suppression setting too: no producer may trust it any longer.
            let role: String = sqlx::query_scalar("select session_user::text")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert_eq!(role, "porchlight_agent");
            sqlx::query("select set_config('porchlight.sync_emitted',$1,true)")
                .bind(json!({format!("{}:reset",m.s.init.household_id):true}).to_string())
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query("select id from porchlight.items where id=$1 for no key update")
                .bind(key)
                .execute(&mut *tx)
                .await
                .unwrap();
            if op == "ancestor_private" {
                sqlx::query(
                    "delete from porchlight.item_assignees where item_id=$1 and member_id=$2",
                )
                .bind(key)
                .bind(m.members[1])
                .execute(&mut *tx)
                .await
                .unwrap();
            }
            let sql = match op {
                "raw_title" => "update porchlight.items set title=title||' raw' where id=$1",
                "ancestor_private" => {
                    "update porchlight.items set visibility='private' where id=$1"
                }
                "ancestor_household" => {
                    "update porchlight.items set visibility='household' where id=$1"
                }
                "ancestor_selected" => {
                    "update porchlight.items set visibility='selected' where id=$1"
                }
                "ancestor_delete" => "update porchlight.items set deleted_at=now() where id=$1",
                "ancestor_restore" => "update porchlight.items set deleted_at=null where id=$1",
                "ancestor_view_gain" => {
                    "insert into porchlight.item_viewers(household_id,item_id,member_id) select household_id,id,$2 from porchlight.items where id=$1"
                }
                "ancestor_view_loss" => {
                    "delete from porchlight.item_viewers where item_id=$1 and member_id=$2"
                }
                _ => unreachable!(),
            };
            let q = sqlx::query(sql).bind(key);
            if op.contains("view_") {
                q.bind(m.members[1]).execute(&mut *tx).await.unwrap();
            } else {
                q.execute(&mut *tx).await.unwrap();
            }
            tx.commit().await.unwrap();
            agent.close().await;
        }
        "availability_source" => {
            let agent = pool_as(&m.pool, Role::Agent, Some("agent:coverage"))
                .await
                .unwrap();
            let mut tx = begin_as(&agent, &Actor::system()).await.unwrap();
            sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00')")
                .bind(m.s.init.household_id).bind(m.members[0]).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
            agent.close().await;
        }
        "member_zone_source" => {
            // No HTTP after-commit callback: commit the production enqueue-only
            // primitive through the app login, then hold the worker. Agents do
            // not have UPDATE/row-lock grants on generated derivation jobs.
            let mut tx = begin_as(&m.s.state.pool, &Actor::system()).await.unwrap();
            let now = *m.clock.lock().unwrap();
            porchlight_db::members::set_time_zone(
                &mut tx,
                m.s.init.household_id,
                m.members[0],
                m.version("household_members", m.members[0]).await,
                chrono_tz::Pacific::Auckland,
                now,
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        "enqueue" => {
            sqlx::query("select porchlight.pl_enqueue_derivation($1,$2)")
                .bind(m.s.init.household_id)
                .bind(m.items[0])
                .execute(&m.pool)
                .await
                .unwrap();
        }
        "actor_sources" | "retention_seed" => {
            let actor = if op == "retention_seed" {
                Actor::system()
            } else {
                Actor {
                    member_id: Some(m.members[0]),
                    ..Actor::system()
                }
            };
            let mut tx = begin_as(&m.pool, &actor).await.unwrap();
            if op == "retention_seed" {
                // Fixture-only history clock. Normal writes cannot set timestamps.
                sqlx::query(
                    "alter table porchlight.notifications disable trigger aa_version_guard",
                )
                .execute(&mut *tx)
                .await
                .unwrap();
            }
            for sources in [1, 2] {
                let n:Uuid = sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,entity_type,entity_id,window_start,release_at,released_at,created_at) values($1,$2,'comment','activity','member',$3,now(),now(),now(),case when $4 then now()-interval '100 days' else now() end) returning id")
                    .bind(m.s.init.household_id).bind(m.members[1]).bind(m.members[0]).bind(op=="retention_seed" && sources==1).fetch_one(&mut *tx).await.unwrap();
                for source in 0..sources {
                    let old = op == "retention_seed" && source == 0;
                    sqlx::query("select set_config('porchlight.notification_now',case when $1 then (now()-interval '100 days')::text else now()::text end,true)").bind(old).execute(&mut *tx).await.unwrap();
                    let e:Uuid = sqlx::query_scalar("select porchlight.pl_emit_notification_event($1,$2,'comment','member',$3,'{}',null,null,null)")
                        .bind(m.s.init.household_id).bind(m.members[1]).bind(m.members[0]).fetch_one(&mut *tx).await.unwrap();
                    sqlx::query("insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)").bind(n).bind(e).execute(&mut *tx).await.unwrap();
                    sqlx::query(
                        "update porchlight.notification_events set processed_at=now() where id=$1",
                    )
                    .bind(e)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                }
            }
            if op == "retention_seed" {
                sqlx::query("alter table porchlight.notifications enable trigger aa_version_guard")
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
            tx.commit().await.unwrap();
        }
        "agenda_move" => {
            sqlx::query("update porchlight.member_agenda set starts_at=starts_at+interval '3 hours',ends_at=ends_at+interval '3 hours',busy=tstzrange(lower(busy)+interval '3 hours',upper(busy)+interval '3 hours','[)') where item_id=$1")
                .bind(m.items[0]).execute(&m.pool).await.unwrap();
        }
        "actor_rename" => {
            sqlx::query("update porchlight.household_members set display_name=display_name||' renamed' where id=$1").bind(m.members[0]).execute(&m.pool).await.unwrap();
        }
        "notification_delete" => {
            sqlx::query("delete from porchlight.notifications where recipient_member_id=$1")
                .bind(m.members[1])
                .execute(&m.pool)
                .await
                .unwrap();
        }
        "source_delete" => {
            sqlx::query("delete from porchlight.notification_sources where notification_id in (select id from porchlight.notifications where recipient_member_id=$1)")
                .bind(m.members[1]).execute(&m.pool).await.unwrap();
        }
        "retention" => {
            sqlx::query("select porchlight.pl_retention($1)")
                .bind(Utc::now())
                .execute(&m.s.state.pool)
                .await
                .unwrap();
        }
        "household_zone" => {
            sqlx::query("update porchlight.households set tzid='Pacific/Auckland' where id=$1")
                .bind(m.s.init.household_id)
                .execute(&m.pool)
                .await
                .unwrap();
        }
        "member_seed" => {
            sqlx::query("insert into porchlight.household_members(household_id,role,display_name,color,tzid) values($1,'dependent','Fixture dependent','#123456','UTC')")
                .bind(m.s.init.household_id).execute(&m.pool).await.unwrap();
        }
        "member_delete" => {
            // An auxiliary dependent has no stream to revoke. The two readers
            // observe a previously cached member disappearing.
            let changed = sqlx::query("update porchlight.household_members set deleted_at=now() where household_id=$1 and display_name='Fixture dependent' and deleted_at is null")
                .bind(m.s.init.household_id)
                .execute(&m.pool)
                .await
                .unwrap();
            assert_eq!(
                changed.rows_affected(),
                1,
                "member deletion fixture must exist"
            );
        }
        _ => panic!("unknown operation {op}"),
    }
}

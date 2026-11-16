//! Operational scheduler work. Fixed kinds/payloads, no paths or credentials in notifications.
use chrono::{DateTime, Utc};
use sqlx::PgConnection;
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime},
};

pub(super) fn certificate_threshold(not_after: SystemTime, now: SystemTime) -> Option<u32> {
    if not_after <= now {
        return Some(0);
    }
    let remaining = not_after.duration_since(now).unwrap_or_default();
    [7, 30, 60, 90]
        .into_iter()
        .find(|days| remaining <= Duration::from_secs(u64::from(*days) * 86400))
}

async fn alert(
    conn: &mut PgConnection,
    kind: &str,
    identity: &str,
    payload: serde_json::Value,
) -> anyhow::Result<()> {
    sqlx::query("select porchlight.pl_emit_notification_event(h.id,m.id,$1,'household',h.id,$3,null,null,$2||':'||m.id::text) from porchlight.households h join porchlight.household_members m on m.household_id=h.id where h.deleted_at is null and m.deleted_at is null and m.role='super_admin'")
        .bind(kind).bind(identity).bind(payload).execute(conn).await?;
    Ok(())
}

pub(super) async fn certificate(
    conn: &mut PgConnection,
    now: DateTime<Utc>,
    root: &Path,
    pinned: &Arc<crate::pki_store::ServerPki>,
) -> anyhow::Result<()> {
    let dir = crate::pki_store::DataDir::new(root);
    let pki = crate::pki_store::load(&dir)?;
    anyhow::ensure!(
        pki.ca.cert().cert_der == pinned.ca.cert().cert_der
            && pki.server_key_pem == pinned.server_key_pem,
        "server PKI identity changed; restart after verified replacement"
    );
    let info = porchlight_pki::parse_cert_pem(&pki.server_cert_pem)?;
    let now_system = SystemTime::from(now);
    if let Some(days) = certificate_threshold(info.not_after, now_system) {
        alert(
            conn,
            match days {
                0 => "server_certificate_expired",
                7 => "server_certificate_expiring_7",
                30 => "server_certificate_expiring_30",
                60 => "server_certificate_expiring_60",
                _ => "server_certificate_expiring_90",
            },
            &format!(
                "server-cert:{}:{days}",
                chrono::DateTime::<Utc>::from(info.not_after).timestamp()
            ),
            serde_json::json!({"threshold_days": days}),
        )
        .await?;
    }
    // Alert commits even when renewal fails: later 30/7-day checks must still reach administrators.
    if crate::pki_store::renew_loaded(&dir, pki, now_system).is_err() {
        tracing::warn!("scheduled server certificate renewal failed; inspect private PKI storage");
        alert(
            conn,
            "server_certificate_renewal_failed",
            &format!(
                "server-cert-renewal:{}:{}",
                chrono::DateTime::<Utc>::from(info.not_after).timestamp(),
                now.date_naive()
            ),
            serde_json::json!({}),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn low_space(
    blocks: u64,
    available: u64,
    inodes: u64,
    free_inodes: u64,
    fragment: u64,
    need: u64,
) -> bool {
    disk_condition(blocks, available, inodes, free_inodes, fragment, need).is_some()
}

fn disk_condition(
    blocks: u64,
    available: u64,
    inodes: u64,
    free_inodes: u64,
    fragment: u64,
    need: u64,
) -> Option<&'static str> {
    if blocks != 0 && u128::from(available) * 100 < u128::from(blocks) * 15 {
        Some("disk_space_low")
    } else if inodes != 0 && u128::from(free_inodes) * 100 < u128::from(inodes) * 15 {
        Some("disk_inodes_low")
    } else if available.saturating_mul(fragment) < need {
        Some("disk_backup_headroom_low")
    } else {
        None
    }
}

pub(super) async fn disk(
    conn: &mut PgConnection,
    now: DateTime<Utc>,
    data: &Path,
    backups: &Path,
) -> anyhow::Result<()> {
    let marker = backups.join(".working-space-need");
    let need = match std::fs::read_to_string(marker) {
        Ok(value) => value.trim().parse::<u64>()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e.into()),
    };
    for (name, root) in [("data", data), ("backups", backups)] {
        let fs = rustix::fs::statvfs(root)?;
        if let Some(kind) = disk_condition(
            fs.f_blocks,
            fs.f_bavail,
            fs.f_files,
            fs.f_favail,
            fs.f_frsize,
            need,
        ) {
            // At most one alert per filesystem class per UTC day, rather than every hourly poll.
            alert(
                conn,
                kind,
                &format!("disk-headroom:{name}:{}", now.date_naive()),
                serde_json::json!({"storage": name, "backup_working_bytes": need}),
            )
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
    async fn operational_alerts_reach_real_admin_inbox(pool: sqlx::PgPool) {
        use axum::{Extension, body::to_bytes};
        use porchlight_core::auth::session::Platform;
        use tower::ServiceExt;
        let server = crate::testing::TestServer::start_with(
            &pool,
            crate::testing::TestOptions {
                background_derivation: false,
                ..Default::default()
            },
        )
        .await;
        let admin = server
            .session_device(server.init.admin_member_id, Platform::Desktop)
            .await;
        let now = Utc::now();
        let mut conn = pool.acquire().await.unwrap();
        let member: uuid::Uuid = sqlx::query_scalar("with u as (insert into porchlight.users(household_id,username) values($1,'member') returning id) insert into porchlight.household_members(household_id,user_id,role,display_name,color,tzid) select $1,id,'user','Sam Rivera','#8EC07C','America/New_York' from u returning id").bind(server.init.household_id).fetch_one(&pool).await.unwrap();
        sqlx::query("insert into porchlight.user_credentials(user_id,household_id,password_hash) select m.user_id,m.household_id,c.password_hash from porchlight.household_members m join porchlight.household_members a on a.household_id=m.household_id and a.role='super_admin' join porchlight.user_credentials c on c.user_id=a.user_id where m.id=$1").bind(member).execute(&pool).await.unwrap();
        let ordinary = server.session_device(member, Platform::Desktop).await;
        let kinds = [
            "server_certificate_expiring_90",
            "server_certificate_expiring_60",
            "server_certificate_expiring_30",
            "server_certificate_expiring_7",
            "server_certificate_expired",
            "server_certificate_renewal_failed",
            "disk_space_low",
            "disk_inodes_low",
            "disk_backup_headroom_low",
        ];
        let router = crate::routes::mtls_router(server.state.clone()).layer(Extension(
            crate::server::ConnInfo {
                transport: crate::server::Transport::Lan,
                client_addr: None,
                client_cert_fingerprint: Some(admin.fingerprint),
            },
        ));
        for kind in kinds {
            alert(
                &mut conn,
                kind,
                kind,
                serde_json::json!({"url":"private payload must not render"}),
            )
            .await
            .unwrap();
            crate::notifier::drain(&server.state.pool, now + chrono::Duration::seconds(1))
                .await
                .unwrap();
            let response = router
                .clone()
                .oneshot(admin.request(
                    "GET",
                    "/v1/notifications?unread=true",
                    serde_json::json!({}),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let body: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(body["items"].as_array().unwrap().len(), 1, "{kind}");
            let summary = &body["items"][0]["summary"];
            assert_eq!(
                summary["category"], "operational",
                "RR-05: {kind} rendered as household activity"
            );
            assert!(
                summary["body"].as_str().unwrap().contains("Check"),
                "{kind}"
            );
            if let Some(days) = kind.strip_prefix("server_certificate_expiring_") {
                assert!(summary["title"].as_str().unwrap().contains(days));
            }
            assert!(!summary.to_string().contains("private payload"));
            sqlx::query(
                "update porchlight.notifications set read_at=now() where recipient_member_id=$1",
            )
            .bind(server.init.admin_member_id)
            .execute(&pool)
            .await
            .unwrap();
        }
        // All conditions in one polling window must remain individually actionable too.
        for kind in kinds {
            alert(
                &mut conn,
                kind,
                &format!("again:{kind}"),
                serde_json::json!({}),
            )
            .await
            .unwrap();
        }
        crate::notifier::drain(&server.state.pool, now + chrono::Duration::seconds(2))
            .await
            .unwrap();
        let response = router
            .oneshot(admin.request(
                "GET",
                "/v1/notifications?unread=true",
                serde_json::json!({}),
                None,
            ))
            .await
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["items"].as_array().unwrap().len(), kinds.len());
        assert!(
            body["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["summary"]["category"] == "operational")
        );
        let router = crate::routes::mtls_router(server.state.clone()).layer(Extension(
            crate::server::ConnInfo {
                transport: crate::server::Transport::Lan,
                client_addr: None,
                client_cert_fingerprint: Some(ordinary.fingerprint),
            },
        ));
        let response = router
            .oneshot(ordinary.request("GET", "/v1/notifications", serde_json::json!({}), None))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert!(body["items"].as_array().unwrap().is_empty());
        drop(conn);
        server.stop().await;
    }
    #[test]
    fn injected_clock_certificate_boundaries() {
        let expiry = SystemTime::UNIX_EPOCH + Duration::from_secs(1000 * 86400);
        for (days, expected) in [
            (91, None),
            (90, Some(90)),
            (61, Some(90)),
            (60, Some(60)),
            (31, Some(60)),
            (30, Some(30)),
            (8, Some(30)),
            (7, Some(7)),
            (0, Some(0)),
        ] {
            assert_eq!(
                certificate_threshold(expiry, expiry - Duration::from_secs(days * 86400)),
                expected
            );
        }
        assert_eq!(
            certificate_threshold(expiry, expiry + Duration::from_secs(1)),
            Some(0)
        );
    }
    #[test]
    fn headroom_covers_bytes_inodes_and_measured_backup() {
        assert!(!low_space(100, 15, 100, 15, 4096, 15 * 4096));
        assert!(low_space(100, 14, 100, 90, 4096, 0));
        assert!(low_space(100, 90, 100, 14, 4096, 0));
        assert!(low_space(100, 90, 100, 90, 4096, 91 * 4096));
        assert!(!low_space(0, 0, 0, 0, 0, 0));
    }
}

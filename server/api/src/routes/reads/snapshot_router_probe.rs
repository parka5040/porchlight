use super::*;
use crate::{
    routes,
    server::{ConnInfo, Transport},
};
use tower::ServiceExt;

#[sqlx::test(migrations = "../db/migrations")]
async fn shipping_gzip_cost(pool: PgPool) {
    let (s, h) = fixture(&pool, 1000).await;
    // Hex digests vary per item and block, unlike repeat('Fixture notes',...).
    sqlx::query("update porchlight.items i set notes=(select string_agg(md5(i.id::text || n::text),'') from generate_series(1,350) n) where household_id=$1").bind(h.id).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.user_credentials(user_id,household_id,password_hash) select m.user_id,m.household_id,c.password_hash from porchlight.household_members m cross join porchlight.user_credentials c join porchlight.household_members owner on owner.user_id=c.user_id where m.household_id=$1 and m.user_id is not null and owner.id=$2").bind(h.id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    let mut devices = Vec::new();
    for member in [h.super_admin, h.super_admin, h.super_admin, h.user] {
        devices.push(
            s.session_device(member, porchlight_core::auth::session::Platform::Desktop)
                .await,
        );
    }
    for concurrent in [1, 4] {
        #[cfg(porchlight_snapshot_probe)]
        let base = super::super::alloc_probe::start();
        let start = Instant::now();
        let requests = devices.iter().take(concurrent).map(|d| {
            let app = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
                transport: Transport::Lan,
                client_addr: None,
                client_cert_fingerprint: Some(d.fingerprint),
            }));
            let mut req = d.request("GET", "/v1/snapshot", json!({}), None);
            req.headers_mut()
                .insert("accept-encoding", "gzip".parse().unwrap());
            async move {
                let r = app.oneshot(req).await.unwrap();
                assert_eq!(r.status(), 200);
                assert_eq!(r.headers()["content-encoding"], "gzip");
                r
            }
        });
        let responses = futures_util::future::join_all(requests).await;
        assert!(
            s.state
                .snapshot_limits
                .acquire(devices[0].device_id, h.super_admin)
                .is_err(),
            "unread incrementally encoded body lost admission"
        );
        let mut frames = Vec::new();
        for r in responses {
            frames.push(body(r).await);
        }
        #[cfg(porchlight_snapshot_probe)]
        let peak = super::super::alloc_probe::peak(base);
        #[cfg(not(porchlight_snapshot_probe))]
        let peak = 0usize;
        let encoded: usize = frames.iter().map(|b| b.len()).sum();
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(
            &mut flate2::read::GzDecoder::new(frames[0].as_ref()),
            &mut decoded,
        )
        .unwrap();
        println!(
            "SNAPSHOT shipping-gzip concurrent={concurrent} identity={} encoded_total={encoded} peak={peak} ms={}",
            decoded.len(),
            start.elapsed().as_millis()
        );
        assert!(decoded.len() > 19_000_000);
        assert!(
            peak <= encoded * 3 / 2 + concurrent * 6 * 1024 * 1024,
            "shipping gzip double-holds identity: peak={peak} encoded={encoded}"
        );
        assert!(
            s.state
                .snapshot_limits
                .acquire(devices[0].device_id, h.super_admin)
                .is_err(),
            "retained encoded frame lost admission"
        );
        drop(frames);
        assert!(
            s.state
                .snapshot_limits
                .acquire(devices[0].device_id, h.super_admin)
                .is_ok()
        );
        super::document_parity::unique(&decoded).unwrap();
        let value: Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(value["items"].as_array().unwrap().len(), 1000);
        assert_eq!(value["occurrences"].as_array().unwrap().len(), 10000);
    }
    s.stop().await;
}

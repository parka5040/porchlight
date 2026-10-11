use super::*;
use crate::{blobs::DiskBudget, concurrency::Limits};
use http_body_util::BodyExt;
use uuid::Uuid;

#[tokio::test(start_paused = true)]
async fn progressing_spools_retain_admission_across_idle_windows_then_release() {
    let root =
        tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var"))
            .unwrap();
    let budget = DiskBudget::default();
    let limits = Limits::new(2, None);
    let household = Uuid::now_v7();
    let mut bodies = Vec::new();
    for _ in 0..2 {
        let permit = limits.acquire(household, household).unwrap();
        let mut disk = budget.reserve(root.path(), 0, 0).unwrap();
        disk.grow(root.path(), 1024 * 1024, 0).unwrap();
        bodies.push(body(
            std::io::Cursor::new(vec![0; 1024 * 1024]),
            Duration::from_secs(30),
            (permit, disk),
        ));
    }
    let reserved = budget.reserved_bytes();
    assert!(reserved >= 2 * 1024 * 1024);
    for _ in 0..4 {
        for body in &mut bodies {
            assert!(body.frame().await.unwrap().is_ok());
        }
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(20)).await;
        assert!(limits.acquire(household, household).is_err());
        assert_eq!(budget.reserved_bytes(), reserved);
    }
    drop(bodies.remove(0));
    tokio::task::yield_now().await;
    assert!(
        budget.reserved_bytes() < reserved,
        "body drop releases retained disk charge"
    );
    tokio::time::advance(Duration::from_secs(31)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        budget.reserved_bytes(),
        0,
        "idle reader releases disk charge"
    );
    assert!(
        bodies.remove(0).collect().await.is_err(),
        "idle export reports incomplete"
    );
    let first = limits.acquire(household, household).unwrap();
    let second = limits.acquire(household, household).unwrap();
    drop((first, second));
}

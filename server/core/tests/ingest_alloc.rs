//! Normal Cargo test target; the slow allocator instrumentation is built only by
//! scripts/check-ingest-alloc.sh (--full). No manual rustc linking or rlib guessing.
//! The explicit cfg keeps unsafe allocator forwarding out of ordinary core targets.
#![allow(unexpected_cfgs)] // porchlight_alloc_probe is supplied/checked by the full-only command.

#[cfg(porchlight_alloc_probe)]
#[path = "support/ingest_alloc_probe.rs"]
mod probe;

#[test]
#[ignore = "slow release allocation measurement; run scripts/check-ingest-alloc.sh"]
fn measured_peaks_and_camera_admission() {
    #[cfg(porchlight_alloc_probe)]
    probe::measured_peaks_and_camera_admission();
    #[cfg(not(porchlight_alloc_probe))]
    panic!("build instrumented release target with scripts/check-ingest-alloc.sh");
}

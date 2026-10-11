use porchlight_api::permissions::{self, PatScope};
#[test]
fn admin_rows_have_pat_scope_boundary() {
    let rows = permissions::matrix_rows(porchlight_api::routes::administration::ENDPOINTS);
    assert_eq!(rows.len(), 90); // per endpoint: two app roles + 3 PAT scopes, self/other
    for row in rows {
        if row.endpoint.policy.label() == "admin PAT" && row.pat_scope == Some(PatScope::Write) {
            assert_eq!(row.status, 403);
        }
    }
}
#[test]
fn last_admin_obeys_r7() {
    let source = include_str!("../../src/routes/administration.rs");
    let part = source
        .split("async fn last_admin(")
        .nth(1)
        .unwrap()
        .split("#[utoipa")
        .next()
        .unwrap();
    assert!(part.contains("for no key update"));
    assert!(!part.contains("for update"));
}
#[test]
fn purge_has_declared_privileges_not_runtime_ddl() {
    let source = include_str!("../../../db/migrations/0014_purge.sql");
    assert!(!source.contains("pl_prepare_purge"));
    assert!(!source.contains("GRANT SELECT, DELETE ON porchlight.notification_events"));
}
#[test]
fn s5_docs_and_valid_gate_key() {
    let readme = include_str!("../../../../README.md");
    assert!(!readme.contains("--token <value>"));
    assert!(readme.contains("porch derive status"));
    let locks = include_str!("../../../../docs/specs/lock-order.md");
    assert!(locks.contains("cross-PAT") && locks.contains("purge_log"));
    let matrix = include_str!("../../../../docs/specs/derivation-matrix.md");
    assert!(matrix.contains("R56") && matrix.contains("purge_log"));
    let gate = include_str!("../../../../deploy/scripts/m3-demo.py");
    assert!(!gate.contains("board_position='Z'"));
    assert!(porchlight_core::ordering::key_after("8080").is_ok());
    assert!(gate.contains("board_position='8080'"));
    assert!(gate.contains("row['board_position'] == '8080'"));
}
#[test]
fn all_agent_operations_declare_pat_security() {
    let api = serde_json::to_value(porchlight_api::openapi::document()).unwrap();
    for endpoint in porchlight_api::routes::administration::ENDPOINTS {
        let op = &api["paths"][endpoint.path][endpoint.method.to_ascii_lowercase()];
        assert_eq!(
            op["security"][0]["pat"],
            serde_json::json!([]),
            "{} {}",
            endpoint.method,
            endpoint.path
        );
    }
}

use super::*;

fn uri() -> EnrollUri {
    EnrollUri {
        host: "example.invalid:8444".into(),
        token: "fixture-enrollment".into(),
        ca_fingerprint: [1; 32],
    }
}

#[test]
fn persisted_identity_contains_only_password_independent_inputs() {
    let uri = uri();
    let pending =
        Pending::new(&uri, "fixture-user", "fixture", "desktop", "fixture secret").unwrap();
    let dir =
        tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var"))
            .unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    pending.save(dir.path()).unwrap();
    let value: Value =
        serde_json::from_slice(&fs::read(dir.path().join("pending.json")).unwrap()).unwrap();
    let allowed = [
        "uri",
        "user",
        "label",
        "platform",
        "csr",
        "key",
        "idempotency_key",
    ];
    assert!(
        value
            .as_object()
            .unwrap()
            .keys()
            .all(|key| allowed.contains(&key.as_str())),
        "pending identity contains password-derived material"
    );
    assert!(
        pending
            .validate(
                &uri,
                "fixture-user",
                "fixture",
                "desktop",
                "different secret"
            )
            .is_ok(),
        "password must be re-supplied to server, not compared to a local verifier"
    );
}

#[test]
fn legacy_attempt_is_sanitized_without_losing_recovery_identity() {
    let dir =
        tempfile::tempdir_in(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var"))
            .unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let original = Pending::new(
        &uri(),
        "fixture-user",
        "fixture",
        "desktop",
        "fixture secret",
    )
    .unwrap();
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy["request_hash"] = json!(vec![7; 32]);
    let path = dir.path().join("pending.json");
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&path, dir.path().join("pending.new")).unwrap();
    let resumed = Pending::load(dir.path()).unwrap();
    assert!(
        resumed.key == original.key
            && resumed.csr == original.csr
            && resumed.idempotency_key == original.idempotency_key
    );
    let saved: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(
        saved.get("request_hash").is_none(),
        "legacy fast verifier remains on disk"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

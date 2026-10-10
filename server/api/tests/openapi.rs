use porchlight_api::{openapi, routes::ROUTES};

#[test]
fn item_queries_and_naive_wall_times_are_precise() {
    let doc = serde_json::to_value(openapi::document()).unwrap();
    for (path, method) in [
        ("/v1/items", "get"),
        ("/v1/items/{id}/occurrences", "get"),
        ("/v1/items/{id}", "patch"),
    ] {
        for param in doc["paths"][path][method]["parameters"].as_array().unwrap() {
            let name = param["name"].as_str().unwrap();
            if [
                "cursor",
                "limit",
                "updated_since",
                "kind",
                "include_deleted",
                "from",
                "to",
                "scope",
                "occurrence_id",
            ]
            .contains(&name)
            {
                assert_eq!(param["in"], "query", "{name}");
                assert_eq!(param["required"], false, "{name}");
            }
        }
    }
    let mut checked = 0;
    for schema in doc["components"]["schemas"].as_object().unwrap().values() {
        if let Some(properties) = schema["properties"].as_object() {
            for (name, field) in properties {
                // A wall-clock value is a string; a non-string `local_*` field (e.g. the boolean
                // `local_alarm`) is not a time and needs no format.
                let string = field["type"] == "string"
                    || field["type"] == serde_json::json!(["string", "null"]);
                if (name.starts_with("local_")
                    || name.starts_with("override_local_")
                    || name == "nominal_local_anchor")
                    && (string || field["format"] == "date-time")
                {
                    assert!(
                        field["type"] == "string"
                            || field["type"] == serde_json::json!(["string", "null"]),
                        "{name}"
                    );
                    assert_eq!(field["format"], "local-date-time", "{name}");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 20);
}
/// Generic over every route (not a name allowlist): each `in: path` parameter appears in its template and each
/// template placeholder is declared as a path parameter, so no query parameter can be mis-declared as a path one.
#[test]
fn path_parameters_and_templates_agree_on_every_route() {
    use std::collections::BTreeSet;
    let doc = serde_json::to_value(openapi::document()).unwrap();
    let mut checked = 0;
    for (path, item) in doc["paths"].as_object().unwrap() {
        let placeholders: BTreeSet<&str> = path
            .split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
            .collect();
        for (method, operation) in item.as_object().unwrap() {
            if !["get", "put", "post", "delete", "patch", "head", "options"]
                .contains(&method.as_str())
            {
                continue;
            }
            let declared: BTreeSet<&str> = operation["parameters"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|param| param["in"] == "path")
                .map(|param| param["name"].as_str().unwrap())
                .collect();
            assert_eq!(declared, placeholders, "{method} {path}");
            checked += 1;
        }
    }
    assert!(checked >= 50, "{checked}");
}
#[test]
fn document_is_current_and_matches_route_registry() {
    let generated = openapi::json();
    assert_eq!(
        generated,
        include_str!("../../../docs/openapi.json"),
        "regenerate: . scripts/env.sh; cd server; CARGO_NET_OFFLINE=true cargo run -q -p porchlight-api --bin porchlight -- openapi > ../docs/openapi.json"
    );
    let doc: serde_json::Value = serde_json::from_str(&generated).unwrap();
    let mut registered = std::collections::BTreeSet::new();
    for route in ROUTES {
        registered.insert((route.path, route.method.to_ascii_lowercase()));
    }
    let mut documented = std::collections::BTreeSet::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for method in item
            .as_object()
            .unwrap()
            .keys()
            .filter(|s| matches!(s.as_str(), "get" | "post" | "put" | "patch" | "delete"))
        {
            documented.insert((path.as_str(), method.to_owned()));
        }
    }
    assert_eq!(registered, documented);
    for (path, method) in registered {
        let operation = &doc["paths"][path][&method];
        assert!(
            operation["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "Porchlight-Client-Version" && p["required"] == true)
        );
        if method != "get" {
            assert!(
                operation["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["name"] == "Idempotency-Key" && p["required"] == true)
            );
        }
    }
}

#[test]
fn headers_and_problem_statuses_are_precise() {
    let doc = serde_json::to_value(openapi::document()).unwrap();
    assert_eq!(doc["info"]["version"], porchlight_core::VERSION);
    assert!(doc["info"].get("license").is_none());
    let mut ids = std::collections::BTreeSet::new();
    let mut operations = std::collections::BTreeSet::new();
    for route in ROUTES {
        if !operations.insert((route.path, route.method)) {
            continue; // Shared mTLS/PAT operation, not a second operationId.
        }
        let op = &doc["paths"][route.path][route.method.to_ascii_lowercase()];
        let id = op["operationId"].as_str().unwrap();
        assert!(ids.insert(id), "duplicate operationId: {id}");
        assert!(id.starts_with(&format!("{}_", route.area)));
        assert_eq!(op["tags"], serde_json::json!([route.area]));
        for response in op["responses"].as_object().unwrap().values() {
            if !matches!(
                route.path,
                "/v1/devices/{id}/revoke" | "/v1/admin/export/household"
            ) {
                assert!(
                    !response["description"]
                        .as_str()
                        .unwrap()
                        .contains("step-up")
                );
            }
        }
        let names: Vec<_> = op["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names.iter().filter(|n| **n == "Idempotency-Key").count(),
            usize::from(route.method != "GET")
        );
        let versioned = versioned_method(route.method)
            || (route.area == "calendar" && route.method == "POST" && route.path.contains("{id}"))
            || (route.area == "items" && route.method == "POST" && route.path != "/v1/items")
            || route.path == "/v1/admin/items/{id}/revert"
            || (route.area == "planning" && matches!(route.handler, "accept" | "dismiss"))
            || (route.area == "lists"
                && route.method == "POST"
                && route.path != "/v1/lists"
                && route.path != "/v1/lists/{id}/entries");
        assert_eq!(names.contains(&"If-Match"), versioned);
        if versioned {
            assert!(
                op["parameters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["name"] == "If-Match" && p["required"] == true)
            );
        }
        assert_eq!(
            names.contains(&"Porchlight-Step-Up"),
            matches!(
                route.path,
                "/v1/devices/{id}/revoke" | "/v1/admin/export/household"
            )
        );
        assert_eq!(op["responses"].get("428").is_some(), versioned);
        assert!(op["responses"].get("405").is_none()); // fallback, not an operation outcome
        for code in ["409", "429"] {
            if let Some(response) = op["responses"].get(code) {
                assert!(response["headers"].get("Retry-After").is_some());
            }
        }
        if route.method == "GET" {
            assert!(op["responses"].get("410").is_none());
            assert!(
                op["responses"].get("409").is_some(),
                "GET auth/touch SQL may return a transient conflict"
            );
            assert!(
                op["responses"]["409"]["headers"]
                    .get("Retry-After")
                    .is_some()
            );
        }
    }
}

fn versioned_method(method: &str) -> bool {
    matches!(method, "PATCH" | "PUT" | "DELETE")
}
#[test]
fn versioned_updates_and_deletes_require_if_match() {
    for method in ["PATCH", "PUT", "DELETE"] {
        assert!(versioned_method(method));
    }
    for method in ["GET", "POST"] {
        assert!(!versioned_method(method));
    }
}

#[test]
fn item_mirror_enums_and_patch_fields_match_core_wire_shapes() {
    use porchlight_core::items as core;
    use serde_json::{Value, json};
    let doc = serde_json::to_value(openapi::document()).unwrap();
    let schemas = &doc["components"]["schemas"];
    let expected = [
        ("ItemKind", json!(core::Kind::ALL)),
        ("ItemTimeMode", json!(core::TimeMode::ALL)),
        ("ItemRecurrenceMode", json!(core::RecurrenceMode::ALL)),
        ("ItemMissedPolicy", json!(core::MissedPolicy::ALL)),
        ("OccurrenceStatus", json!(core::Status::ALL)),
        (
            "ItemVisibility",
            json!(porchlight_core::access::Visibility::ALL),
        ),
        (
            "ItemCapability",
            json!(porchlight_core::access::Capability::ALL),
        ),
    ];
    for (name, core) in expected {
        let strings = |v: &Value| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(
            strings(&schemas[name]["enum"]),
            strings(&core),
            "wire enum {name}"
        );
    }
    let core = serde_json::to_value(porchlight_db::items::ItemWrite::default()).unwrap();
    let names = |v: &Value| {
        v.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    };
    let keys = names(&core);
    assert_eq!(names(&schemas["ItemFields"]["properties"]), keys);
    let mut patch = names(&schemas["PatchRequest"]["properties"]);
    patch.remove("scope");
    patch.remove("occurrence_id");
    assert_eq!(patch, keys);
}

use porchlight_api::{
    permissions::{self, Relationship},
    testing::{HouseholdFixture, TestClient, agent_request, trust},
};
use porchlight_core::items::Status;
use porchlight_db::{
    items::{self, ItemWrite, Relationships},
    tx::{Actor, begin_as},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[test]
fn endpoint_registry_and_matrix_are_complete_and_current() {
    let expected = permissions::permissions_markdown();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/specs/permissions-matrix.md");
    assert!(
        std::fs::read_to_string(path).is_ok_and(|actual| actual == expected),
        "regenerate API permissions-matrix example"
    );
    for surface in [
        porchlight_api::routes::Surface::Mtls,
        porchlight_api::routes::Surface::Agent,
    ] {
        let routes: Vec<_> = porchlight_api::routes::ROUTES
            .iter()
            .filter(|r| r.area == "items" && r.surface == surface)
            .collect();
        assert_eq!(
            routes.len(),
            permissions::ITEM_ENDPOINTS
                .iter()
                .filter(|e| e.case.is_empty())
                .count()
        );
        for endpoint in permissions::ITEM_ENDPOINTS {
            assert!(routes.iter().any(|r| r.method == endpoint.method
                && r.path == endpoint.path
                && r.handler == endpoint.handler));
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn every_endpoint_visibility_relationship_runs_through_http(pool: PgPool) {
    let f = HouseholdFixture::build(&pool).await;
    let actor = Actor {
        member_id: Some(f.admin.member_id),
        device_id: Some(f.admin.device.device_id),
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let rows = permissions::matrix_rows(permissions::ITEM_ENDPOINTS);
    assert_eq!(rows.len(), 330);
    for row in rows {
        let who = if matches!(
            row.relationship,
            Relationship::SuperAdmin | Relationship::Agent
        ) {
            &f.admin
        } else {
            &f.users[0]
        };
        let title = format!("secret-{}", Uuid::now_v7());
        let fields = ItemWrite {
            title: title.clone(),
            visibility: row.visibility.as_str().into(),
            owner_id: if matches!(row.relationship, Relationship::Owner) {
                who.member_id
            } else {
                f.users[1].member_id
            },
            subject_member_id: if row.context.unassigned_dependent_item {
                Some(f.dependent)
            } else if matches!(row.relationship, Relationship::Subject) {
                Some(who.member_id)
            } else {
                None
            },
            timing: porchlight_core::items::SeriesInput {
                kind: porchlight_core::items::Kind::Appointment,
                time_mode: porchlight_core::items::TimeMode::Fixed,
                tzid: Some("UTC".into()),
                starts_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                ends_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
                ..Default::default()
            },
            ..Default::default()
        };
        let sets = Relationships {
            assignees: if matches!(row.relationship, Relationship::Assignee) {
                vec![who.member_id]
            } else {
                vec![]
            },
            viewers: if matches!(row.relationship, Relationship::Viewer) {
                vec![who.member_id]
            } else {
                vec![]
            },
            ..Default::default()
        };
        let mut tx = begin_as(&f.server.state.pool, &actor).await.unwrap();
        let other_title = format!("counterpart-{}", Uuid::now_v7());
        let other = items::create(
            &mut tx,
            f.server.init.household_id,
            &ItemWrite {
                title: other_title.clone(),
                visibility: "private".into(),
                owner_id: f.users[1].member_id,
                subject_member_id: Some(items::agenda_members(&fields, &sets)[0]),
                timing: fields.timing.clone(),
                ..Default::default()
            },
            &Relationships::default(),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut tx = begin_as(&f.server.state.pool, &actor).await.unwrap();
        let made = items::create(
            &mut tx,
            f.server.init.household_id,
            &fields,
            &sets,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut conn = f.server.state.pool.acquire().await.unwrap();
        let item = porchlight_db::derive::load_item(&mut conn, made.item_id)
            .await
            .unwrap();
        let occ = porchlight_db::derive::occurrences(&mut conn, made.item_id)
            .await
            .unwrap()
            .remove(0);
        drop(conn);
        // Every row probes actual conflict rendering before its mutation removes busy time.
        let read_path = format!("/v1/items/{}", item.id);
        let req = who.device.request("GET", &read_path, json!({}), None);
        let probe = if matches!(row.relationship, Relationship::Agent) {
            agent_request(
                &f.server.agent_socket,
                Some(f.server.bootstrap_token()),
                req,
            )
            .await
        } else {
            TestClient::new(&f.server.ca_pem, Some(&who.device))
                .request(f.server.lan, req)
                .await
                .unwrap()
        };
        if porchlight_core::access::can_view(&row.context) {
            assert_eq!(probe.status, 200);
            let conflicts = probe.json()["conflicts"].as_array().unwrap().clone();
            assert!(!conflicts.is_empty(), "no real conflicts: {row:?}");
            let conflict = conflicts
                .iter()
                .find(|c| c["other_title"] == other_title || c["other_title"] == "Busy")
                .unwrap();
            if matches!(row.relationship, Relationship::Agent) {
                assert_eq!(conflict["other_item_id"], other.item_id.to_string());
            } else {
                assert_eq!(conflict["other_title"], "Busy");
                assert!(conflict["other_item_id"].is_null());
                assert!(conflict["other_occurrence_id"].is_null());
                assert!(!String::from_utf8_lossy(&probe.body).contains(&other_title));
            }
        } else {
            assert_eq!(probe.status, 404);
        }
        if row.endpoint.handler == "reopen" {
            let mut tx = begin_as(&f.server.state.pool, &actor).await.unwrap();
            items::set_status(
                &mut tx,
                f.server.init.household_id,
                occ.id,
                occ.version,
                Status::Done,
                Some(f.admin.member_id),
                chrono::Utc::now(),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
        let occurrence_route = row.endpoint.path.starts_with("/v1/occurrences/");
        let path = row.endpoint.path.replace(
            "{id}",
            &if occurrence_route { occ.id } else { item.id }.to_string(),
        );
        let body = match row.endpoint.handler {
            "create" => json!({"title":"created"}),
            "patch" => json!({"title":"edited"}),
            "assignees" => json!({"assignees":[who.member_id]}),
            "viewers" => json!({"viewers":[who.member_id]}),
            "reminders" => json!({"reminder_offsets":[5]}),
            "patch_occurrence" => match row.endpoint.case {
                "override" => {
                    json!({"override_local_start": "2026-10-07T14:00:00", "override_local_end":"2026-10-07T15:00:00"})
                }
                "board" => json!({"board_move":{}}),
                _ => json!({"status":"doing"}),
            },
            _ => json!({}),
        };
        let mut req = who.device.request(row.endpoint.method, &path, body, None);
        let version = if occurrence_route {
            sqlx::query_scalar::<_, i32>(
                "SELECT version FROM porchlight.item_occurrences WHERE id=$1",
            )
            .bind(occ.id)
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap()
        } else {
            item.version
        };
        req.headers_mut()
            .insert("if-match", version.to_string().parse().unwrap());
        let result = if matches!(row.relationship, Relationship::Agent) {
            agent_request(
                &f.server.agent_socket,
                Some(f.server.bootstrap_token()),
                req,
            )
            .await
        } else {
            TestClient::new(&f.server.ca_pem, Some(&who.device))
                .request(f.server.lan, req)
                .await
                .unwrap()
        };
        assert_eq!(
            result.status,
            row.status,
            "{:?}: {}",
            row,
            String::from_utf8_lossy(&result.body)
        );
        if row.endpoint.handler == "list" {
            let body = result.json();
            let listed = body["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["id"] == item.id.to_string());
            let present = listed.is_some();
            if let Some(listed) = listed {
                assert_eq!(
                    listed["conflicts"],
                    probe.json()["conflicts"],
                    "list/GET conflict parity {row:?}"
                );
                let conflicts = listed["conflicts"].as_array().unwrap();
                assert!(!conflicts.is_empty(), "list lost real conflicts {row:?}");
                for conflict in conflicts {
                    if matches!(row.relationship, Relationship::Agent) {
                        assert_eq!(conflict["other_item_id"], other.item_id.to_string());
                        assert!(!conflict["other_occurrence_id"].is_null());
                    } else {
                        assert_eq!(conflict["other_title"], "Busy");
                        assert!(conflict["other_item_id"].is_null());
                        assert!(conflict["other_occurrence_id"].is_null());
                    }
                }
            }
            if !matches!(row.relationship, Relationship::Agent) {
                assert!(
                    !String::from_utf8_lossy(&result.body).contains(&other_title),
                    "list leaked counterpart {row:?}"
                );
            }
            assert_eq!(
                present,
                porchlight_core::access::can_view(&row.context),
                "collection membership {row:?}"
            );
        }
        if row.status >= 400 && row.endpoint.method != "GET" {
            let unchanged: i32 = if occurrence_route {
                sqlx::query_scalar("SELECT version FROM porchlight.item_occurrences WHERE id=$1")
                    .bind(occ.id)
                    .fetch_one(&f.server.state.pool)
                    .await
                    .unwrap()
            } else {
                sqlx::query_scalar("SELECT version FROM porchlight.items WHERE id=$1")
                    .bind(item.id)
                    .fetch_one(&f.server.state.pool)
                    .await
                    .unwrap()
            };
            assert_eq!(version, unchanged, "denied write changed version {row:?}");
        }
        if row.status >= 400
            || (row.endpoint.handler == "list" && !porchlight_core::access::can_view(&row.context))
        {
            assert!(
                !String::from_utf8_lossy(&result.body).contains(&title),
                "forbidden title leaked: {row:?}"
            );
        }
        let mut tx = begin_as(&f.server.state.pool, &actor).await.unwrap();
        sqlx::query(
            "UPDATE porchlight.items SET deleted_at=now() WHERE id=ANY($1) AND deleted_at IS NULL",
        )
        .bind(vec![item.id, other.item_id])
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    // Explicit UD=true case: a visible unrelated user may take; visibility still dominates.
    for visibility in ["household", "private", "selected"] {
        let made = trust::admin(
            &f.server,
            "POST",
            "/v1/items",
            json!({"title":"child task","visibility":visibility,"subject_member_id":f.dependent}),
        )
        .await;
        assert_eq!(made.status, 201);
        let item = made.json();
        let path = format!("/v1/items/{}/take", item["id"].as_str().unwrap());
        let mut req = f.users[0].device.request("POST", &path, json!({}), None);
        req.headers_mut()
            .insert("if-match", item["version"].to_string().parse().unwrap());
        let result = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device))
            .request(f.server.lan, req)
            .await
            .unwrap();
        assert_eq!(
            result.status,
            if visibility == "household" { 200 } else { 404 }
        );
        if visibility == "household" {
            assert_eq!(result.json()["assignees"], json!([f.users[0].member_id]));
        }
    }
    f.server.stop().await;
}

#[test]
fn framework_prose_is_area_and_row_count_independent() {
    let doc = permissions::permissions_markdown();
    assert!(!doc.contains("(S4a)"));
    assert!(!doc.contains("base 273"));
}

#[test]
fn non_item_policies_are_pluggable_and_ignore_item_visibility() {
    use permissions::{Endpoint, Policy, PolicyContext};
    use porchlight_core::access::CallerKind;
    fn self_only(c: &PolicyContext) -> u16 {
        if c.is_self { 200 } else { 403 }
    }
    fn admin_only(c: &PolicyContext) -> u16 {
        if matches!(
            c.item.caller,
            CallerKind::Agent | CallerKind::AppMember(porchlight_core::access::Role::SuperAdmin)
        ) {
            200
        } else {
            403
        }
    }
    for (label, expected) in [
        ("self", self_only as fn(&PolicyContext) -> u16),
        ("admin", admin_only),
    ] {
        let endpoint = Endpoint {
            method: "GET",
            path: "/v1/example/{id}",
            handler: "example",
            policy: Policy::Custom { label, expected },
            case: "",
        };
        for row in permissions::matrix_rows(&[endpoint]) {
            let wanted = if label == "self" {
                row.is_self
            } else {
                matches!(
                    row.context.caller,
                    CallerKind::AppMember(porchlight_core::access::Role::SuperAdmin)
                        | CallerKind::Agent
                )
            };
            assert_eq!(row.status, if wanted { 200 } else { 403 });
        }
    }
}

#[test]
fn pat_scope_hierarchy_is_central_and_closed() {
    use porchlight_api::extract::pat_allows_scope;
    for (scopes, write, admin) in [
        (&[][..], false, false),
        (&["read"][..], false, false),
        (&["write"][..], true, false),
        (&["admin"][..], true, true),
    ] {
        let scopes = scopes.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(pat_allows_scope(&scopes, "read"));
        assert_eq!(pat_allows_scope(&scopes, "write"), write);
        assert_eq!(pat_allows_scope(&scopes, "admin"), admin);
        assert!(!pat_allows_scope(&scopes, "unknown"));
    }
}

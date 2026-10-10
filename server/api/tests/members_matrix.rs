use porchlight_api::{
    permissions::{self, Relationship},
    testing::{HouseholdFixture, TestClient, agent_request, trust},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

fn member_endpoints() -> Vec<permissions::Endpoint> {
    permissions::READ_ENDPOINTS
        .iter()
        .copied()
        .filter(|e| {
            e.path == "/v1/members" || e.path == "/v1/members/{id}" || e.path == "/v1/me/settings"
        })
        .collect()
}

#[test]
fn generated_member_rows_include_self_for_every_caller_kind() {
    let endpoints = member_endpoints();
    let rows = permissions::matrix_rows(&endpoints);
    for relationship in [
        Relationship::Other,
        Relationship::SuperAdmin,
        Relationship::Agent,
    ] {
        assert!(
            rows.iter().any(|row| {
                row.endpoint.method == "PATCH"
                    && row.endpoint.case.is_empty()
                    && row.relationship.as_str() == relationship.as_str()
                    && row.status == 200
            }),
            "missing successful self row for {relationship:?}"
        );
    }
    assert!(endpoints.iter().any(|e| e.case == "authority"));
    assert_eq!(rows.len(), endpoints.len() * 10);
    for row in rows {
        assert!(!row.item_dimension);
        assert!(
            !row.context.is_owner
                && !row.context.is_assignee
                && !row.context.is_viewer
                && !row.context.is_subject
        );
        let read_only_mutation =
            row.endpoint.method != "GET" && row.pat_scope == Some(permissions::PatScope::Read);
        let forbidden_target = row.endpoint.method == "PATCH"
            && (!row.is_self
                || (row.endpoint.case == "authority"
                    && matches!(
                        row.context.caller,
                        porchlight_core::access::CallerKind::Agent
                    )));
        assert_eq!(
            row.status,
            if read_only_mutation || forbidden_target {
                403
            } else {
                200
            },
            "{row:?}"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn every_generated_member_settings_row_runs_through_http(pool: PgPool) {
    let f = HouseholdFixture::build(&pool).await;
    let mut pats = Vec::new();
    for scope in permissions::PatScope::ALL {
        let scopes = scope.scopes();
        let refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
        let (_, pat) = trust::seed_pat(&pool, &f.server, &refs).await;
        pats.push((scope, pat));
    }
    let rows = permissions::matrix_rows(&member_endpoints());
    assert_eq!(
        rows.len(),
        50,
        "five member/settings cases × five caller/scope kinds × self/other"
    );
    for row in rows {
        let who = if matches!(
            row.relationship,
            Relationship::SuperAdmin | Relationship::Agent
        ) {
            &f.admin
        } else {
            &f.users[0]
        };
        assert!(!row.item_dimension);
        assert!(!row.context.is_owner);
        let target = if row.is_self {
            who.member_id
        } else {
            f.users[1].member_id
        };
        let path = row.endpoint.path.replace("{id}", &target.to_string());
        let version: i32 = match row.endpoint.method {
            "PATCH" => {
                sqlx::query_scalar("SELECT version FROM porchlight.household_members WHERE id=$1")
                    .bind(target)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
            "PUT" => sqlx::query_scalar(
                "SELECT version FROM porchlight.member_settings WHERE member_id=$1",
            )
            .bind(who.member_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            _ => 1,
        };
        let body = match (row.endpoint.method, row.endpoint.case) {
            ("PATCH", "authority") => json!({"make_zone_authority":true}),
            ("PATCH", "") => json!({"color":"#123ABC"}),
            ("PUT", "") => {
                json!({"today_sections":["overdue","today","this_week","needs_owner"],"scope":"mine","this_week_days":7,"pinned_list_id":null})
            }
            ("GET", "") => json!({}),
            other => panic!("unhandled member/settings case {other:?}"),
        };
        let mut request = who.device.request(
            row.endpoint.method,
            &path,
            body,
            Some(&Uuid::now_v7().to_string()),
        );
        request
            .headers_mut()
            .insert("if-match", version.to_string().parse().unwrap());
        let response = if matches!(row.relationship, Relationship::Agent) {
            let pat = &pats
                .iter()
                .find(|(scope, _)| Some(*scope) == row.pat_scope)
                .unwrap()
                .1;
            agent_request(&f.server.agent_socket, Some(pat), request).await
        } else {
            TestClient::new(&f.server.ca_pem, Some(&who.device))
                .request(f.server.lan, request)
                .await
                .unwrap()
        };
        assert_eq!(response.status, row.status, "generated row {row:?}");
        if response.status == 200 && row.endpoint.path == "/v1/me/settings" {
            assert_eq!(response.json()["member_id"], who.member_id.to_string());
        }
        if response.status == 200 && row.endpoint.method == "PATCH" {
            assert_eq!(response.json()["id"], who.member_id.to_string());
            if row.endpoint.case == "authority" {
                assert_eq!(
                    response.json()["tz_authority_device_id"],
                    who.device.device_id.to_string()
                );
                assert_eq!(response.json()["tz_source"], "device");
            }
        }
        if row.endpoint.method == "PUT" && response.status == 200 {
            let other_version: i32 = sqlx::query_scalar(
                "SELECT version FROM porchlight.member_settings WHERE member_id=$1",
            )
            .bind(f.users[1].member_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                other_version, 1,
                "settings route must never retarget another member"
            );
        }
    }
    f.server.stop().await;
}

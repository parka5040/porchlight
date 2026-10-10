use porchlight_api::{
    calendar::permissions::CALENDAR_ENDPOINTS,
    permissions::{self, PatScope, Relationship},
    testing::{HouseholdFixture, TestClient, TestOptions, agent_request, trust},
};
use porchlight_core::access::{CallerKind, Role};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[test]
fn calendar_policies_use_self_and_exact_pat_scope() {
    for row in permissions::matrix_rows(CALENDAR_ENDPOINTS) {
        let admin = matches!(row.context.caller, CallerKind::AppMember(Role::SuperAdmin))
            || row.pat_scope == Some(PatScope::Admin);
        let self_route =
            row.endpoint.path == "/v1/imports/ics" || row.endpoint.path == "/v1/admin/feeds/target";
        let allowed = (admin || (self_route && row.is_self))
            && !(row.endpoint.method != "GET" && row.pat_scope == Some(PatScope::Read));
        let expected = if !allowed {
            403
        } else if row.endpoint.method == "POST" && row.endpoint.path.ends_with("refresh") {
            202
        } else if row.endpoint.method == "POST" && row.endpoint.path.ends_with("feeds") {
            201
        } else {
            200
        };
        assert_eq!(row.status, expected, "{row:?}");
        assert!(
            !row.item_dimension,
            "calendar policies do not depend on item visibility"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn every_applicable_calendar_matrix_row_runs_through_http(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let mut pats = Vec::new();
    for scope in PatScope::ALL {
        let scopes = scope.scopes();
        let refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
        pats.push((scope, trust::seed_pat(&pool, &f.server, &refs).await.1));
    }
    let mut tested = 0;
    let rows = permissions::matrix_rows(CALENDAR_ENDPOINTS);
    assert_eq!(
        rows.len(),
        120,
        "twelve calendar cases × five caller/scope kinds × self/other"
    );
    for row in rows {
        let agent = matches!(row.relationship, Relationship::Agent);
        // Matrix policy rows describe both caller types; invoke only mounted surfaces.
        if (row.endpoint.path.starts_with("/v1/admin/") && !agent)
            || (row.endpoint.path.starts_with("/v1/feeds") && agent)
        {
            continue;
        }
        let who = if matches!(
            row.relationship,
            Relationship::SuperAdmin | Relationship::Agent
        ) {
            &f.admin
        } else {
            &f.users[0]
        };
        let target = if row.is_self {
            who.member_id
        } else {
            f.users[1].member_id
        };
        let mut path = row.endpoint.path.to_owned();
        let mut version = 1;
        if path.contains("{id}") {
            let source = trust::admin(&f.server,"POST","/v1/admin/feeds",json!({"name":"matrix","url":"https://example.invalid/calendar","target_member_id":target})).await;
            assert_eq!(source.status, 201);
            let source = source.json();
            path = path.replace("{id}", source["id"].as_str().unwrap());
            version = source["version"].as_i64().unwrap();
        }
        let body = if path == "/v1/imports/ics" {
            json!({"content":"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:matrix\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Matrix\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n","member_id":target,"scope":Uuid::now_v7().to_string(),"preview":true})
        } else if path == "/v1/admin/feeds/target" {
            let username: String = sqlx::query_scalar("select u.username from porchlight.users u join porchlight.household_members m on m.user_id=u.id where m.id=$1").bind(target).fetch_one(&pool).await.unwrap();
            path = format!("{path}?username={username}");
            json!({})
        } else if row.endpoint.method == "PUT"
            || (row.endpoint.method == "POST" && path.ends_with("feeds"))
        {
            json!({"name":"matrix","url":"https://example.invalid/calendar","target_member_id":target})
        } else {
            json!({})
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
        let response = if agent {
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
        assert_eq!(response.status, row.status, "{row:?}");
        tested += 1;
    }
    assert_eq!(
        tested, 66,
        "all mounted surface/caller combinations covered"
    );
    f.server.stop().await;
}

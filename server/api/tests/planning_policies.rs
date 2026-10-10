use porchlight_api::{
    permissions::{self, Relationship},
    planning_permissions,
    testing::{HouseholdFixture, TestClient, TestOptions, agent_request, trust},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;
fn options() -> TestOptions {
    TestOptions {
        background_derivation: false,
        item_clock: Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
        ..Default::default()
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn suggestion_actions_and_candidates_http_permission_matrix(pool: PgPool) {
    use porchlight_core::items::{Kind, SeriesInput, TimeMode};
    use porchlight_db::{
        items::{self, ItemWrite, Relationships},
        tx::{Actor, begin_as},
    };
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    let mut pats = Vec::new();
    for scope in permissions::PatScope::ALL {
        let (_, token) = trust::seed_pat(&pool, s, &[scope.as_str()]).await;
        pats.push((scope, token));
    }
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    let mut tested = 0;
    for row in permissions::matrix_rows(planning_permissions::ENDPOINTS)
        .into_iter()
        .filter(|r| matches!(r.endpoint.handler, "accept" | "dismiss" | "candidates"))
    {
        let who = if matches!(
            row.relationship,
            Relationship::SuperAdmin | Relationship::Agent
        ) {
            &f.admin
        } else {
            &f.users[0]
        };
        let owner = if matches!(row.relationship, Relationship::Owner) {
            who.member_id
        } else {
            f.users[1].member_id
        };
        let is_candidates = row.endpoint.handler == "candidates";
        let target_member = if row.is_self {
            who.member_id
        } else {
            f.users[1].member_id
        };
        let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
        let fields = ItemWrite {
            owner_id: owner,
            title: "Matrix planning item".into(),
            visibility: row.visibility.as_str().into(),
            subject_member_id: if is_candidates {
                Some(f.dependent)
            } else if matches!(row.relationship, Relationship::Subject) {
                Some(who.member_id)
            } else {
                Some(target_member)
            },
            est_minutes: if is_candidates { None } else { Some(30) },
            timing: SeriesInput {
                kind: if is_candidates {
                    Kind::Appointment
                } else {
                    Kind::Todo
                },
                time_mode: TimeMode::Floating,
                local_start: is_candidates.then(|| "2026-10-12T10:00:00".parse().unwrap()),
                local_end: is_candidates.then(|| "2026-10-12T11:00:00".parse().unwrap()),
                due_date: (!is_candidates).then(|| "2026-10-12".parse().unwrap()),
                ..Default::default()
            },
            ..Default::default()
        };
        // Candidate eligibility requires no assignees. An assignee receives View
        // even for private items; mirror that ACL with ownership for this case.
        let fields = if is_candidates && matches!(row.relationship, Relationship::Assignee) {
            ItemWrite {
                owner_id: who.member_id,
                ..fields
            }
        } else {
            fields
        };
        let sets = Relationships {
            assignees: if !is_candidates && matches!(row.relationship, Relationship::Assignee) {
                vec![who.member_id]
            } else if !is_candidates
                && matches!(row.relationship, Relationship::Subject)
                && !row.is_self
            {
                vec![target_member]
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
        let item = items::create(&mut tx, s.init.household_id, &fields, &sets, now)
            .await
            .unwrap()
            .item_id;
        tx.commit().await.unwrap();
        let occ: (Uuid, i32) =
            sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1")
                .bind(item)
                .fetch_one(&pool)
                .await
                .unwrap();
        let (path, version) = if is_candidates {
            (format!("/v1/items/{item}/candidates"), occ.1)
        } else {
            let member = target_member;
            let id: Uuid = sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,'2026-10-12T13:00:00Z','2026-10-12T13:30:00Z',$5) returning id").bind(s.init.household_id).bind(member).bind(item).bind(occ.0).bind(Uuid::now_v7()).fetch_one(&pool).await.unwrap();
            (
                format!("/v1/suggestions/{id}/{}", row.endpoint.handler),
                if row.endpoint.handler == "accept" {
                    occ.1
                } else {
                    1
                },
            )
        };
        let key = Uuid::now_v7().to_string();
        let mut req = if matches!(row.relationship, Relationship::Agent) {
            trust::req(row.endpoint.method, &path, json!({}), &key)
        } else {
            who.device
                .request(row.endpoint.method, &path, json!({}), Some(&key))
        };
        req.headers_mut()
            .insert("if-match", version.to_string().parse().unwrap());
        let r = if matches!(row.relationship, Relationship::Agent) {
            agent_request(
                &s.agent_socket,
                Some(
                    pats.iter()
                        .find(|(scope, _)| Some(*scope) == row.pat_scope)
                        .unwrap()
                        .1
                        .as_str(),
                ),
                req,
            )
            .await
        } else {
            TestClient::new(&s.ca_pem, Some(&who.device))
                .request(s.lan, req)
                .await
                .unwrap()
        };
        assert_eq!(
            r.status,
            row.status,
            "{} {:?} {:?}: {}",
            row.endpoint.handler,
            row.relationship,
            row.visibility,
            r.json()
        );
        tested += 1;
    }
    assert_eq!(tested, 129);
    f.server.stop().await;
}

#[test]
fn planning_registry_rows_are_complete() {
    for surface in [
        porchlight_api::routes::Surface::Mtls,
        porchlight_api::routes::Surface::Agent,
    ] {
        let rows: Vec<_> = porchlight_api::routes::ROUTES
            .iter()
            .filter(|r| r.area == "planning" && r.surface == surface)
            .collect();
        assert_eq!(rows.len(), 7);
        for e in planning_permissions::ENDPOINTS {
            assert!(
                rows.iter()
                    .any(|r| r.method == e.method && r.path == e.path && r.handler == e.handler)
            )
        }
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn member_policies_http_matrix_self_admin_agents_and_privacy(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    let mut pats = Vec::new();
    for scope in permissions::PatScope::ALL {
        let (_, token) = trust::seed_pat(&pool, s, &[scope.as_str()]).await;
        pats.push((scope, token));
    }
    let mut tested = 0;
    for row in permissions::matrix_rows(planning_permissions::ENDPOINTS)
        .into_iter()
        .filter(|r| {
            matches!(
                r.endpoint.handler,
                "availability" | "put_availability" | "suggest" | "suggestions"
            )
        })
    {
        let who = if matches!(
            row.relationship,
            Relationship::SuperAdmin | Relationship::Agent
        ) {
            &f.admin
        } else {
            &f.users[0]
        };
        let member = if row.endpoint.case == "dependent" {
            f.dependent
        } else if row.is_self {
            who.member_id
        } else {
            f.users[1].member_id
        };
        let path = match row.endpoint.handler {
            "suggest" => "/v1/schedule/suggest".into(),
            "suggestions" => format!("/v1/suggestions?member_id={member}"),
            _ => format!("/v1/members/{member}/availability"),
        };
        let body = if row.endpoint.handler == "suggest" {
            json!({"member_id":member})
        } else if row.endpoint.method == "PUT" {
            json!({"windows":[]})
        } else {
            json!({})
        };
        let version: Option<i32> = sqlx::query_scalar(
            "select version from porchlight.availability_profiles where member_id=$1",
        )
        .bind(member)
        .fetch_optional(&pool)
        .await
        .unwrap();
        let key = Uuid::now_v7().to_string();
        let mut req = if matches!(row.relationship, Relationship::Agent) {
            trust::req(row.endpoint.method, &path, body, &key)
        } else {
            who.device
                .request(row.endpoint.method, &path, body, Some(&key))
        };
        req.headers_mut().insert(
            "if-match",
            version.unwrap_or(0).to_string().parse().unwrap(),
        );
        let r = if matches!(row.relationship, Relationship::Agent) {
            agent_request(
                &s.agent_socket,
                Some(
                    pats.iter()
                        .find(|(scope, _)| Some(*scope) == row.pat_scope)
                        .unwrap()
                        .1
                        .as_str(),
                ),
                req,
            )
            .await
        } else {
            TestClient::new(&s.ca_pem, Some(&who.device))
                .request(s.lan, req)
                .await
                .unwrap()
        };
        assert_eq!(
            r.status,
            row.status,
            "{} {} {:?} {:?} self={}: {}",
            row.endpoint.handler,
            row.endpoint.case,
            row.relationship,
            row.pat_scope,
            row.is_self,
            r.json()
        );
        tested += 1;
    }
    assert_eq!(tested, 50);
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn dependent_ranking_hidden_admin_and_nightly_proposal_replacement(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    for member in [
        f.admin.member_id,
        f.users[0].member_id,
        f.users[1].member_id,
    ] {
        sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','17:00')").bind(s.init.household_id).bind(member).execute(&pool).await.unwrap();
    }
    let client = TestClient::new(&s.ca_pem, Some(&f.users[0].device));
    let made=client.request(s.lan,f.users[0].device.request("POST","/v1/items",json!({"title":"Dependent visit","kind":"appointment","time_mode":"fixed","tzid":"America/New_York","starts_at":"2026-10-12T14:00:00Z","ends_at":"2026-10-12T15:00:00Z","subject_member_id":f.dependent,"visibility":"private"}),None)).await.unwrap();
    assert_eq!(made.status, 201, "{}", made.json());
    let path = format!(
        "/v1/items/{}/candidates",
        made.json()["id"].as_str().unwrap()
    );
    let ranked = client
        .request(
            s.lan,
            f.users[0].device.request("GET", &path, json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(ranked.status, 200, "{}", ranked.json());
    assert_eq!(ranked.json()["candidates"].as_array().unwrap().len(), 3);
    assert!(
        ranked.json()["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["reason"] == "free_inside_availability")
    );
    let admin = TestClient::new(&s.ca_pem, Some(&f.admin.device))
        .request(s.lan, f.admin.device.request("GET", &path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(admin.status, 404);
    assert!(!String::from_utf8_lossy(&admin.body).contains("Dependent visit"));
    let agent = trust::agent(
        s,
        Some(s.bootstrap_token()),
        "GET",
        &path,
        json!({}),
        "candidate",
    )
    .await;
    assert_eq!(agent.status, 200);
    let busy=client.request(s.lan,f.users[0].device.request("POST","/v1/items",json!({"title":"Busy","kind":"appointment","time_mode":"fixed","tzid":"America/New_York","starts_at":"2026-10-12T14:00:00Z","ends_at":"2026-10-12T15:00:00Z","assignees":[f.users[1].member_id]}),None)).await.unwrap();
    assert_eq!(busy.status, 201);
    let ranked = client
        .request(
            s.lan,
            f.users[0].device.request("GET", &path, json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(
        ranked.json()["candidates"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["member_id"],
        f.users[1].member_id.to_string()
    );
    assert_eq!(
        ranked.json()["candidates"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["reason"],
        "conflicting"
    );
    let task=client.request(s.lan,f.users[0].device.request("POST","/v1/items",json!({"title":"Planned","due_date":"2026-10-12","est_minutes":30,"visibility":"private"}),None)).await.unwrap();
    assert_eq!(task.status, 201, "{}", task.json());
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    porchlight_api::planning::nightly(&s.state.pool, now)
        .await
        .unwrap();
    let first: Uuid = sqlx::query_scalar(
        "select id from porchlight.schedule_suggestions where member_id=$1 and status='proposed'",
    )
    .bind(f.users[0].member_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    porchlight_api::planning::nightly(&s.state.pool, now)
        .await
        .unwrap();
    let next: Uuid = sqlx::query_scalar(
        "select id from porchlight.schedule_suggestions where member_id=$1 and status='proposed'",
    )
    .bind(f.users[0].member_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(first, next);
    let accept = format!("/v1/suggestions/{next}/accept");
    let mut req = f.admin.device.request("POST", &accept, json!({}), None);
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let hidden = TestClient::new(&s.ca_pem, Some(&f.admin.device))
        .request(s.lan, req)
        .await
        .unwrap();
    assert_eq!(hidden.status, 404);
    let suggestions = client
        .request(
            s.lan,
            f.users[0]
                .device
                .request("GET", "/v1/suggestions?limit=1", Value::Null, None),
        )
        .await
        .unwrap();
    assert_eq!(suggestions.json()["items"].as_array().unwrap().len(), 1);
    let id = task.json()["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let occ: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("update porchlight.item_occurrences set status='doing' where id=$1")
        .bind(occ)
        .execute(&pool)
        .await
        .unwrap();
    let mut req = f.users[0]
        .device
        .request("POST", &accept, json!({}), Some("stale-occurrence"));
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let stale = client.request(s.lan, req).await.unwrap();
    assert_eq!(stale.status, 409);
    assert_eq!(stale.json()["current"]["id"], occ.to_string());
    let detached: bool =
        sqlx::query_scalar("select is_detached from porchlight.item_occurrences where id=$1")
            .bind(occ)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!detached);
    f.server.stop().await;
}

use porchlight_api::{
    lists_policy,
    permissions::{self, PatScope, Relationship},
    routes::{self, Surface},
    testing::{HouseholdFixture, TestClient, TestOptions, agent_request, trust},
};
use porchlight_db::{
    items::{self, ItemWrite, Relationships},
    tx::{Actor, begin_as},
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
#[test]
fn lists_routes_have_policy_rows_on_both_surfaces() {
    for surface in [Surface::Mtls, Surface::Agent] {
        let routes: Vec<_> = routes::ROUTES
            .iter()
            .filter(|r| r.area == "lists" && r.surface == surface)
            .collect();
        assert_eq!(routes.len(), 12);
        for route in routes {
            assert!(!route.legacy);
            assert!(lists_policy::ENDPOINTS.iter().any(|e| e.case.is_empty()
                && e.method == route.method
                && e.path == route.path
                && e.handler == route.handler));
        }
    }
}
#[test]
fn list_rows_preserve_acl_scope_and_independent_self_dimensions() {
    let rows = permissions::matrix_rows(lists_policy::ENDPOINTS);
    assert_eq!(rows.len(), lists_policy::ENDPOINTS.len() * 54);
    for endpoint in lists_policy::ENDPOINTS {
        for visibility in porchlight_core::access::Visibility::ALL {
            for relationship in Relationship::ALL {
                let scopes = if matches!(relationship, Relationship::Agent) {
                    PatScope::ALL.iter().copied().map(Some).collect::<Vec<_>>()
                } else {
                    vec![None]
                };
                for pat_scope in scopes {
                    for is_self in [true, false] {
                        let row = rows
                            .iter()
                            .find(|row| {
                                row.endpoint.handler == endpoint.handler
                                    && row.endpoint.case == endpoint.case
                                    && row.visibility == visibility
                                    && row.relationship.as_str() == relationship.as_str()
                                    && row.pat_scope == pat_scope
                                    && row.is_self == is_self
                            })
                            .expect("missing list ACL/scope/self row");
                        assert!(row.item_dimension);
                        assert_eq!(
                            row.context.is_owner,
                            matches!(relationship, Relationship::Owner)
                        );
                        assert_eq!(
                            row.context.is_viewer,
                            matches!(relationship, Relationship::Viewer)
                        );
                        if pat_scope == Some(PatScope::Read) && endpoint.method != "GET" {
                            assert_eq!(row.status, 403, "read-scope mutation");
                        }
                        let other = rows
                            .iter()
                            .find(|other| {
                                other.endpoint.handler == endpoint.handler
                                    && other.endpoint.case == endpoint.case
                                    && other.visibility == visibility
                                    && other.relationship.as_str() == relationship.as_str()
                                    && other.pat_scope == pat_scope
                                    && other.is_self != is_self
                            })
                            .unwrap();
                        assert_eq!(row.status, other.status, "self does not change list ACL");
                    }
                }
            }
        }
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn every_list_endpoint_visibility_relationship_and_attachment_through_http(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let actor = Actor {
        member_id: Some(f.admin.member_id),
        device_id: Some(f.admin.device.device_id),
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let mut pats = Vec::new();
    for scope in PatScope::ALL {
        let scopes = scope.scopes();
        let refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
        let (_, pat) = trust::seed_pat(&pool, &f.server, &refs).await;
        pats.push((scope, pat));
    }
    for row in permissions::matrix_rows(lists_policy::ENDPOINTS) {
        let who = if matches!(
            row.relationship,
            Relationship::SuperAdmin | Relationship::Agent
        ) {
            &f.admin
        } else {
            &f.users[0]
        };
        let owner = if row.context.is_owner {
            who.member_id
        } else {
            f.users[1].member_id
        };
        let mut tx = begin_as(&pool, &actor).await.unwrap();
        let item = if row.endpoint.case == "attached" {
            let fields = ItemWrite {
                title: "Attached checklist".into(),
                visibility: row.visibility.as_str().into(),
                owner_id: owner,
                subject_member_id: row.context.is_subject.then_some(who.member_id),
                ..Default::default()
            };
            let sets = Relationships {
                assignees: if row.context.is_assignee {
                    vec![who.member_id]
                } else {
                    vec![]
                },
                viewers: if row.context.is_viewer {
                    vec![who.member_id]
                } else {
                    vec![]
                },
                ..Default::default()
            };
            Some(
                items::create(
                    &mut tx,
                    f.server.init.household_id,
                    &fields,
                    &sets,
                    chrono::DateTime::parse_from_rfc3339("2026-10-05T12:00:00Z")
                        .unwrap()
                        .to_utc(),
                )
                .await
                .unwrap()
                .item_id,
            )
        } else {
            None
        };
        let list: Uuid=sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,item_id,owner_id,name,visibility) VALUES($1,$2,$3,'Policy list',$4) RETURNING id").bind(f.server.init.household_id).bind(item).bind(owner).bind(if item.is_some(){"household"}else{row.visibility.as_str()}).fetch_one(&mut *tx).await.unwrap();
        if row.context.is_viewer && item.is_none() {
            sqlx::query("INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) VALUES($1,$2,$3)").bind(f.server.init.household_id).bind(list).bind(who.member_id).execute(&mut *tx).await.unwrap();
        }
        let entry: Uuid=sqlx::query_scalar("INSERT INTO porchlight.list_entries(household_id,list_id,label) VALUES($1,$2,'Policy entry') RETURNING id").bind(f.server.init.household_id).bind(list).fetch_one(&mut *tx).await.unwrap();
        let version: i32 = sqlx::query_scalar("SELECT version FROM porchlight.lists WHERE id=$1")
            .bind(list)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let sql_read: bool = sqlx::query_scalar("SELECT porchlight.pl_list_can_read($1,$2)")
            .bind(list)
            .bind(who.member_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        if !matches!(row.relationship, Relationship::Agent) {
            let context = if item.is_some() {
                row.context
            } else {
                porchlight_core::access::standalone_list_context(
                    row.context.caller,
                    row.context.visibility,
                    row.context.is_owner,
                    row.context.is_viewer,
                )
            };
            let rust_read = porchlight_core::access::capabilities(&context)
                .contains(porchlight_core::access::Capability::View);
            assert_eq!(
                sql_read, rust_read,
                "SQL/Rust read agreement: {:?} {:?} {}",
                row.relationship, row.visibility, row.endpoint.case
            );
        }
        tx.commit().await.unwrap();
        let target = if row.endpoint.path.contains("list-entries") {
            entry
        } else {
            list
        };
        let path = row.endpoint.path.replace("{id}", &target.to_string());
        let body = match row.endpoint.handler {
            "create" => json!({"name":"New list","item_id":item}),
            "patch" => json!({"name":"Updated"}),
            "add_entry" => json!({"label":"Extra"}),
            "patch_entry" => json!({"label":"Edited"}),
            "viewers" => json!({"viewers":[]}),
            _ => json!({}),
        };
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "if-match",
            if target == entry {
                "1".parse().unwrap()
            } else {
                version.to_string().parse().unwrap()
            },
        );
        let response = if matches!(row.relationship, Relationship::Agent) {
            let mut req = who.device.request(row.endpoint.method, &path, body, None);
            req.headers_mut().extend(headers);
            let (_, pat) = pats
                .iter()
                .find(|(scope, _)| Some(*scope) == row.pat_scope)
                .unwrap();
            agent_request(&f.server.agent_socket, Some(pat), req).await
        } else {
            let mut req = who.device.request(row.endpoint.method, &path, body, None);
            req.headers_mut().extend(headers);
            TestClient::new(&f.server.ca_pem, Some(&who.device))
                .request(f.server.lan, req)
                .await
                .unwrap()
        };
        assert_eq!(
            response.status, row.status,
            "handler={} case={} rel={:?} vis={:?}",
            row.endpoint.handler, row.endpoint.case, row.relationship, row.visibility
        );
    }
    f.server.stop().await;
}

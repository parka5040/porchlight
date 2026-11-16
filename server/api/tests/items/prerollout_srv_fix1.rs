use porchlight_api::{
    extract::Caller,
    items,
    routes::items::OccurrencesQuery,
    testing::{TestOptions, TestServer},
};
use porchlight_db::derive;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn mixed_generated_override_hull_matches_full_materialization(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let caller = Caller::Agent {
        household_id: s.init.household_id,
        member_id: s.init.admin_member_id,
        token_id: Uuid::nil(),
        label: "Hull probe".into(),
        scopes: vec!["admin".into()],
        request_id: Uuid::nil(),
    };
    let item: Uuid = sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,time_mode,due_at,tzid) VALUES($1,$2,'Mixed anchors','deadline','fixed','2026-01-01Z','UTC') RETURNING id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq,local_due,due_at,override_local_due,override_local_start,override_local_end) SELECT $1,$2,n,'2026-01-01','2026-01-01Z','2030-01-01','2030-01-01','2030-01-02' FROM generate_series(0,2) n")
        .bind(s.init.household_id).bind(item).execute(&pool).await.unwrap();
    sqlx::query("UPDATE porchlight.items SET time_mode='floating',tzid=NULL,due_at=NULL,local_due='2026-01-01' WHERE id=$1").bind(item).execute(&pool).await.unwrap();
    let from = "2028-01-01T00:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let to = from + chrono::Duration::days(1);
    let mut conn = pool.acquire().await.unwrap();
    let spec = derive::load_item(&mut conn, item)
        .await
        .unwrap()
        .spec()
        .unwrap();
    let full = derive::occurrences(&mut conn, item).await.unwrap();
    let mut expected: Vec<_> = full
        .iter()
        .filter(|o| {
            let a = derive::effective_anchor(o, &spec).unwrap();
            let start = a
                .starts_at
                .or(a.due_at)
                .or(a
                    .local_start
                    .or(a.local_due)
                    .map(|l| porchlight_core::time::resolve_local(l, chrono_tz::UTC)))
                .unwrap();
            let end = a
                .ends_at
                .or(a
                    .local_end
                    .map(|l| porchlight_core::time::resolve_local(l, chrono_tz::UTC)))
                .unwrap_or(start);
            start < to && end > from
        })
        .map(|o| (o.anchor.seq, o.id))
        .collect();
    expected.sort();
    assert_eq!(expected.len(), 3, "legacy mixed-anchor membership");
    drop(conn);
    let mut cursor = None;
    let mut got = Vec::new();
    loop {
        let page = items::occurrences_with_zone(
            &s.state,
            &caller,
            item,
            OccurrencesQuery {
                from: Some(from),
                to: Some(to),
                undated: None,
                cursor,
                limit: Some(1),
            },
            Some(chrono_tz::UTC),
        )
        .await
        .unwrap();
        got.extend(page.items.iter().map(|o| (o.seq, o.id)));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        got, expected,
        "SQL hull must retain mixed generated/override anchors"
    );
    s.stop().await;
}

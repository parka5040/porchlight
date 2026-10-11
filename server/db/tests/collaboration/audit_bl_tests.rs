use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn final_notification_kind_catalog_and_original_producer_abi(pool: PgPool) {
    let checks: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_constraint c JOIN pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=ANY(c.conkey) WHERE c.conrelid='porchlight.notification_events'::regclass AND c.contype='c' AND a.attname='kind'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(
        checks, 0,
        "kind membership belongs only to the extensible FK"
    );
    let fk: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_constraint c WHERE c.conrelid='porchlight.notification_events'::regclass AND c.confrelid='porchlight.notification_kinds'::regclass AND c.contype='f' AND c.convalidated AND c.conkey=ARRAY[(SELECT attnum FROM pg_attribute WHERE attrelid=c.conrelid AND attname='kind')] AND c.confkey=ARRAY[(SELECT attnum FROM pg_attribute WHERE attrelid=c.confrelid AND attname='kind')]")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(fk, 1, "exact validated registry FK survives all migrations");
    let (body, security_definer, config, names, args, defaults, result): (String, bool, Vec<String>, Vec<String>, String, i16, String) = sqlx::query_as("SELECT prosrc,prosecdef,proconfig,proargnames,oidvectortypes(proargtypes),pronargdefaults,pg_get_function_result(oid) FROM pg_proc WHERE oid='porchlight.pl_emit_notification_event(uuid,uuid,text,text,uuid,jsonb,uuid,uuid,text)'::regprocedure")
        .fetch_one(&pool).await.unwrap();
    // Compare the defining migration with the final catalog, not two snapshots
    // surrounding fixture INSERTs (which cannot detect a later ABI replacement).
    let defining = include_str!("../../migrations/0015_items_reads.sql")
        .split_once("CREATE FUNCTION pl_emit_notification_event(")
        .unwrap()
        .1;
    let original_body = defining
        .split_once("AS $$")
        .unwrap()
        .1
        .split_once("$$;")
        .unwrap()
        .0;
    assert_eq!(body, original_body);
    assert!(security_definer);
    assert_eq!(config, vec!["search_path=pg_catalog, porchlight, pg_temp"]);
    assert_eq!(
        names,
        [
            "p_household",
            "p_recipient",
            "p_kind",
            "p_entity_type",
            "p_entity_id",
            "p_payload",
            "p_item",
            "p_occurrence",
            "p_dedupe_key"
        ]
    );
    assert_eq!(
        args,
        "uuid, uuid, text, text, uuid, jsonb, uuid, uuid, text"
    );
    assert_eq!(defaults, 4);
    assert_eq!(result, "uuid");
}

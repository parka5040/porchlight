use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn renewal_protects_deleted_attachment_and_never_revives_expired_lease(pool: PgPool) {
    let h: Uuid = sqlx::query_scalar(
        "insert into porchlight.households(name,tzid) values('Lease test','UTC') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let m: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Dependent','dependent','#123456','UTC') returning id").bind(h).fetch_one(&pool).await.unwrap();
    let i: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,title,kind,owner_id) values($1,'Attachment','todo',$2) returning id").bind(h).bind(m).fetch_one(&pool).await.unwrap();
    let sha = "a".repeat(64);
    sqlx::query("insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,$2,5,'text/plain')").bind(h).bind(&sha).execute(&pool).await.unwrap();
    let attachment: Uuid = sqlx::query_scalar("insert into porchlight.attachments(household_id,item_id,uploader_id,original_name,blob_sha256) values($1,$2,$3,'note.txt',$4) returning id").bind(h).bind(i).bind(m).bind(&sha).fetch_one(&pool).await.unwrap();
    let lease: Uuid = sqlx::query_scalar("select porchlight.pl_take_blob_lease('backup:test')")
        .fetch_one(&pool)
        .await
        .unwrap();
    let renewed: bool =
        sqlx::query_scalar("select porchlight.pl_renew_blob_lease($1,interval '12 hours')")
            .bind(lease)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(renewed);
    sqlx::query("delete from porchlight.attachments where id=$1")
        .bind(attachment)
        .execute(&pool)
        .await
        .unwrap();
    let err = sqlx::query("delete from porchlight.blobs where household_id=$1 and sha256=$2")
        .bind(h)
        .bind(&sha)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some("23503")
    );
    sqlx::query("update porchlight.blob_leases set created_at='2000-01-01',expires_at=clock_timestamp()-interval '1 second' where id=$1").bind(lease).execute(&pool).await.unwrap();
    let revived: bool = sqlx::query_scalar("select porchlight.pl_renew_blob_lease($1)")
        .bind(lease)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!revived);
    assert_eq!(
        sqlx::query("delete from porchlight.blobs where household_id=$1 and sha256=$2")
            .bind(h)
            .bind(&sha)
            .execute(&pool)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
}

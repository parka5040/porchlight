//! Full household fixture for later server workstreams; device enrollment exercises the HTTP contract.
use super::{TestDevice, TestServer};
use porchlight_core::auth::session::Platform;
use porchlight_db::tx::{Actor, begin_as};
use sqlx::PgPool;
use uuid::Uuid;
pub struct FixtureUser {
    pub user_id: Uuid,
    pub member_id: Uuid,
    pub device: TestDevice,
}
pub struct HouseholdFixture {
    pub server: TestServer,
    pub admin: FixtureUser,
    pub users: [FixtureUser; 2],
    pub dependent: Uuid,
}
impl HouseholdFixture {
    pub async fn build(pool: &PgPool) -> Self {
        Self::build_with(pool, super::TestOptions::default()).await
    }
    pub async fn build_with(pool: &PgPool, options: super::TestOptions) -> Self {
        let server = TestServer::start_with(pool, options).await;
        let admin_device = server
            .enroll("admin", &server.admin_password, Platform::Desktop)
            .await;
        let user_id =
            sqlx::query_scalar("SELECT user_id FROM porchlight.household_members WHERE id=$1")
                .bind(server.init.admin_member_id)
                .fetch_one(&server.state.pool)
                .await
                .unwrap();
        let admin = FixtureUser {
            user_id,
            member_id: server.init.admin_member_id,
            device: admin_device,
        };
        let mut users = Vec::new();
        for (username, name) in [("alice", "Alice"), ("bob", "Bob")] {
            let (user_id, member_id) =
                super::trust::user_named(&server.state.pool, &server, username, name).await;
            let device = server
                .enroll(username, &server.admin_password, Platform::Desktop)
                .await;
            users.push(FixtureUser {
                user_id,
                member_id,
                device,
            });
        }
        let mut tx = begin_as(
            &server.state.pool,
            &Actor {
                member_id: Some(admin.member_id),
                device_id: Some(admin.device.device_id),
                request_id: Uuid::now_v7(),
                agent_label: None,
                token_id: None,
            },
        )
        .await
        .unwrap();
        let dependent=sqlx::query_scalar("INSERT INTO porchlight.household_members(household_id,role,display_name,color,tzid) VALUES ($1,'dependent','Child','#AABBCC','America/New_York') RETURNING id").bind(server.init.household_id).fetch_one(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let [alice, bob]: [FixtureUser; 2] =
            users.try_into().unwrap_or_else(|_| panic!("two users"));
        Self {
            server,
            admin,
            users: [alice, bob],
            dependent,
        }
    }
}

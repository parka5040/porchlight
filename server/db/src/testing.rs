//! Test helpers: connect to a sqlx::test database as one of the Porchlight login roles.

use anyhow::Context;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgConnection, PgPool};

/// Task-local observations of the planning producer batches; no SQL/budget overhead.
/// Each test scope is isolated even when SQLx runs databases concurrently.
#[derive(Default, Debug)]
pub struct PlanningHooks {
    pub loads: Vec<Vec<crate::planning::LoadTransition>>,
    pub suggestions: Vec<(uuid::Uuid, uuid::Uuid, Option<uuid::Uuid>)>,
}
tokio::task_local! {
    static PLANNING_HOOKS: std::cell::RefCell<PlanningHooks>;
}
pub async fn with_planning_hooks<F: std::future::Future>(future: F) -> (F::Output, PlanningHooks) {
    PLANNING_HOOKS
        .scope(std::cell::RefCell::new(PlanningHooks::default()), async {
            let output = future.await;
            (output, PLANNING_HOOKS.with(|hooks| hooks.take()))
        })
        .await
}
pub(crate) fn record_load_hook(transitions: &[crate::planning::LoadTransition]) {
    let _ = PLANNING_HOOKS.try_with(|h| h.borrow_mut().loads.push(transitions.to_vec()));
}
pub(crate) fn record_suggestion_hook(
    member: uuid::Uuid,
    run: uuid::Uuid,
    item: Option<uuid::Uuid>,
) {
    let _ = PLANNING_HOOKS.try_with(|h| h.borrow_mut().suggestions.push((member, run, item)));
}

#[derive(Clone, Copy, Debug)]
pub enum Role {
    Migrator,
    App,
    Agent,
}

impl Role {
    fn credentials(self) -> anyhow::Result<(&'static str, String)> {
        let (user, var) = match self {
            Role::Migrator => ("porchlight_migrator", "PORCHLIGHT_MIGRATOR_PASSWORD"),
            Role::App => ("porchlight_app", "PORCHLIGHT_APP_PASSWORD"),
            Role::Agent => ("porchlight_agent", "PORCHLIGHT_AGENT_PASSWORD"),
        };
        let password = std::env::var(var)
            .with_context(|| format!("{var} is not set; source scripts/env.sh"))?;
        Ok((user, password))
    }
}

/// Connection options for the pool's database as a Porchlight login role.
pub fn connect_options_as(
    pool: &PgPool,
    role: Role,
    application_name: Option<&str>,
) -> anyhow::Result<PgConnectOptions> {
    let (user, password) = role.credentials()?;
    let mut options = (*pool.connect_options())
        .clone()
        .username(user)
        .password(&password);
    if let Some(name) = application_name {
        options = options.application_name(name);
    }
    Ok(options)
}

/// Opens a connection to the pool's database as `role`, optionally with an `application_name`.
pub async fn connect_as(
    pool: &PgPool,
    role: Role,
    application_name: Option<&str>,
) -> anyhow::Result<PgConnection> {
    Ok(connect_options_as(pool, role, application_name)?
        .connect()
        .await?)
}

/// A pool (at most two connections) on the pool's database as `role`, for code under test that takes a pool.
pub async fn pool_as(
    pool: &PgPool,
    role: Role,
    application_name: Option<&str>,
) -> anyhow::Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(2)
        .connect_with(connect_options_as(pool, role, application_name)?)
        .await?)
}

/// Ids of a household created by [`seed_household`]; member ids, not user ids.
#[derive(Clone, Copy, Debug)]
pub struct Household {
    pub id: uuid::Uuid,
    pub super_admin: uuid::Uuid,
    pub user: uuid::Uuid,
    pub dependent: uuid::Uuid,
}

/// Inserts a household with a super admin, a regular user and a dependent (run as the pool's superuser).
pub async fn seed_household(pool: &PgPool) -> anyhow::Result<Household> {
    let mut tx = pool.begin().await?;
    let id: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.households (name, tzid) values ('Test Household', 'America/New_York') returning id",
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut ids = Vec::new();
    for (username, role, name, color) in [
        (Some("admin"), "super_admin", "Sam", "#FABD2F"),
        (Some("alex"), "user", "Alex", "#8EC07C"),
        (None, "dependent", "Theo", "#D3869B"),
    ] {
        let user_id: Option<uuid::Uuid> = match username {
            Some(u) => Some(
                sqlx::query_scalar(
                    "insert into porchlight.users (household_id, username) values ($1, $2) returning id",
                )
                .bind(id)
                .bind(u)
                .fetch_one(&mut *tx)
                .await?,
            ),
            None => None,
        };
        let member: uuid::Uuid = sqlx::query_scalar(
            "insert into porchlight.household_members (household_id, user_id, role, display_name, color, tzid) \
             values ($1, $2, $3, $4, $5, 'America/New_York') returning id",
        )
        .bind(id)
        .bind(user_id)
        .bind(role)
        .bind(name)
        .bind(color)
        .fetch_one(&mut *tx)
        .await?;
        ids.push(member);
    }
    tx.commit().await?;
    Ok(Household {
        id,
        super_admin: ids[0],
        user: ids[1],
        dependent: ids[2],
    })
}

/// Inserts a fixed-time appointment "Dentist" owned by the super admin, 2026-10-12 14:00-15:00 America/New_York.
pub async fn insert_appointment(pool: &PgPool, h: &Household) -> anyhow::Result<uuid::Uuid> {
    let id = sqlx::query_scalar(
        "insert into porchlight.items (household_id, owner_id, title, kind, time_mode, tzid, starts_at, ends_at) \
         values ($1, $2, 'Dentist', 'appointment', 'fixed', 'America/New_York', \
                 '2026-10-12 14:00'::timestamp at time zone 'America/New_York', \
                 '2026-10-12 15:00'::timestamp at time zone 'America/New_York') returning id",
    )
    .bind(h.id)
    .bind(h.super_admin)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

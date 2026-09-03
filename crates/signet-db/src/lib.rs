use chrono::{DateTime, Utc};
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

pub use sqlx::{FromRow, PgPool};

pub static MIGRATOR: Migrator = sqlx::migrate!();

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

#[derive(FromRow, Debug, Clone)]
pub struct EnvironmentRow {
    pub id: Uuid,
    pub name: String,
    pub npub_owner: String,
    pub workspace_id: Option<Uuid>,
    pub status: String,
    pub block_policy: String,
    pub signet_challenge: String,
    pub component_explorer: bool,
    pub component_indexer: bool,
    pub component_faucet: bool,
    pub component_lightning: Option<String>,
    pub rpc_endpoint: String,
    pub indexer_endpoint: Option<String>,
    pub explorer_endpoint: Option<String>,
    pub faucet_endpoint: Option<String>,
    pub ln_endpoint: Option<String>,
    pub ttl_secs: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub current_snapshot_id: Option<Uuid>,
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database: {0}")]
    Sql(#[from] sqlx::Error),
}

pub struct NewEnvironment<'a> {
    pub id: Uuid,
    pub name: &'a str,
    pub npub_owner: &'a str,
    pub block_policy: &'a str,
    pub signet_challenge: &'a str,
    pub component_explorer: bool,
    pub component_indexer: bool,
    pub component_faucet: bool,
    pub rpc_endpoint: &'a str,
    pub ttl_secs: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub async fn create_environment(
    pool: &PgPool,
    env: &NewEnvironment<'_>,
) -> Result<EnvironmentRow, DbError> {
    sqlx::query_as::<_, EnvironmentRow>(
        r#"
        insert into environments (
            id, name, npub_owner, status, block_policy, signet_challenge,
            component_explorer, component_indexer, component_faucet,
            rpc_endpoint, ttl_secs, expires_at
        )
        values ($1, $2, $3, 'provisioning', $4, $5, $6, $7, $8, $9, $10, $11)
        returning *
        "#,
    )
    .bind(env.id)
    .bind(env.name)
    .bind(env.npub_owner)
    .bind(env.block_policy)
    .bind(env.signet_challenge)
    .bind(env.component_explorer)
    .bind(env.component_indexer)
    .bind(env.component_faucet)
    .bind(env.rpc_endpoint)
    .bind(env.ttl_secs)
    .bind(env.expires_at)
    .fetch_one(pool)
    .await
    .map_err(DbError::from)
}

pub async fn get_environment(pool: &PgPool, id: Uuid) -> Result<Option<EnvironmentRow>, DbError> {
    sqlx::query_as::<_, EnvironmentRow>("select * from environments where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(DbError::from)
}

pub async fn set_environment_status(pool: &PgPool, id: Uuid, status: &str) -> Result<(), DbError> {
    sqlx::query("update environments set status = $2 where id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

#[derive(FromRow, Debug, Clone)]
pub struct ApiTokenRow {
    pub npub_owner: String,
    pub revoked: bool,
}

pub async fn find_api_token(
    pool: &PgPool,
    token_hash: &str,
) -> Result<Option<ApiTokenRow>, sqlx::Error> {
    sqlx::query_as::<_, ApiTokenRow>(
        "select npub_owner, (revoked_at is not null) as revoked from api_tokens where token_hash = $1",
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await
}

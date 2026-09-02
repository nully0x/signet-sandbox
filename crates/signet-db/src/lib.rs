use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

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

#[derive(FromRow)]
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

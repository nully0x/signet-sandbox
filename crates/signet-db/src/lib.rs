use chrono::{DateTime, Utc};
use serde_json::Value;
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
    pub rpc_endpoint: String,
    pub indexer_endpoint: Option<String>,
    pub explorer_endpoint: Option<String>,
    pub faucet_endpoint: Option<String>,
    pub ln_endpoint: Option<String>,
    pub ttl_secs: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub current_snapshot_id: Option<Uuid>,
    pub versions: Option<Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("port pool: {0}")]
    Pool(String),
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
    pub explorer_endpoint: Option<&'a str>,
    pub indexer_endpoint: Option<&'a str>,
    pub electrum_port: Option<i32>,
    pub ttl_secs: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub versions: Option<Value>,
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
            rpc_endpoint, explorer_endpoint, indexer_endpoint, electrum_port, ttl_secs,
            expires_at, versions
        )
        values ($1, $2, $3, 'provisioning', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
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
    .bind(env.explorer_endpoint)
    .bind(env.indexer_endpoint)
    .bind(env.electrum_port)
    .bind(env.ttl_secs)
    .bind(env.expires_at)
    .bind(env.versions.clone())
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

pub async fn get_environment_by_name(
    pool: &PgPool,
    npub_owner: &str,
    name: &str,
) -> Result<Option<EnvironmentRow>, DbError> {
    sqlx::query_as::<_, EnvironmentRow>(
        "select * from environments where npub_owner = $1 and name = $2",
    )
    .bind(npub_owner)
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(DbError::from)
}

pub async fn list_environments(
    pool: &PgPool,
    npub_owner: &str,
) -> Result<Vec<EnvironmentRow>, DbError> {
    sqlx::query_as::<_, EnvironmentRow>(
        "select * from environments where npub_owner = $1 order by created_at desc",
    )
    .bind(npub_owner)
    .fetch_all(pool)
    .await
    .map_err(DbError::from)
}

pub fn is_unique_violation(err: &DbError) -> bool {
    matches!(
        err,
        DbError::Sql(sqlx::Error::Database(db)) if db.code().as_deref() == Some("23505")
    )
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

/// Mark an environment expired by its 12-hex namespace short id; the reaper
/// knows namespace names, not uuids. Returns how many rows changed.
pub async fn mark_expired_by_short_id(pool: &PgPool, short_id: &str) -> Result<u64, DbError> {
    let result = sqlx::query(
        "update environments set status = 'expired'
         where left(replace(id::text, '-', ''), 12) = $1 and status <> 'destroyed'",
    )
    .bind(short_id)
    .execute(pool)
    .await
    .map_err(DbError::from)?;
    Ok(result.rows_affected())
}

#[derive(FromRow, Debug, Clone)]
pub struct LightningInstanceRow {
    pub name: String,
    pub implementation: String,
    pub port: Option<i32>,
}

/// Replace the lightning instance list for one environment. The create
/// path is the only caller; names are validated upstream.
pub async fn set_lightning_instances(
    pool: &PgPool,
    env_id: Uuid,
    instances: &[(signet_core::env::LightningInstance, i32)],
) -> Result<(), DbError> {
    let mut tx = pool.begin().await.map_err(DbError::from)?;
    sqlx::query("delete from environment_lightning where environment_id = $1")
        .bind(env_id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    for (instance, port) in instances {
        sqlx::query(
            "insert into environment_lightning (environment_id, name, implementation, port) \
             values ($1, $2, $3, $4)",
        )
        .bind(env_id)
        .bind(&instance.name)
        .bind(instance.implementation.as_str())
        .bind(port)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

pub async fn get_lightning_instances(
    pool: &PgPool,
    env_id: Uuid,
) -> Result<Vec<LightningInstanceRow>, DbError> {
    sqlx::query_as::<_, LightningInstanceRow>(
        "select name, implementation, port from environment_lightning \
         where environment_id = $1 order by name",
    )
    .bind(env_id)
    .fetch_all(pool)
    .await
    .map_err(DbError::from)
}

// Ports held by live environments' lightning instances: each instance
// occupies its REST port and the gRPC port right after it.
async fn held_lightning_ports(pool: &PgPool) -> Result<Vec<i32>, DbError> {
    let ports: Vec<i32> = sqlx::query_scalar(
        "select el.port from environment_lightning el \
         join environments e on e.id = el.environment_id \
         where el.port is not null and e.status in ('provisioning', 'ready', 'stopped')",
    )
    .fetch_all(pool)
    .await
    .map_err(DbError::from)?;
    Ok(ports.into_iter().flat_map(|p| [p, p + 1]).collect())
}

// The lowest even-aligned port in `[base, max]` whose pair (p, p + 1) is
// free given `held`.
fn free_port_pair(base: i32, max: i32, held: &[i32]) -> Option<i32> {
    let mut p = if base % 2 == 0 { base } else { base + 1 };
    while p < max {
        if !held.contains(&p) && !held.contains(&(p + 1)) {
            return Some(p);
        }
        p += 2;
    }
    None
}

/// Allocate one REST/gRPC port pair per requested instance inside the
/// published lightning range. Ports held by live environments stay
/// reserved; destroyed and expired environments release theirs.
pub async fn next_lightning_ports(
    pool: &PgPool,
    base: i32,
    max: i32,
    pairs: usize,
) -> Result<Vec<i32>, DbError> {
    let held = held_lightning_ports(pool).await?;
    let mut allocated = Vec::with_capacity(pairs);
    let mut all_held = held;
    for _ in 0..pairs {
        match free_port_pair(base, max, &all_held) {
            Some(p) => {
                all_held.push(p);
                all_held.push(p + 1);
                allocated.push(p);
            }
            None => return Err(DbError::Pool("lightning port pool exhausted".into())),
        }
    }
    Ok(allocated)
}

// Sequential allocation inside the published electrum port range: the
// lowest port not held by a live environment. Destroyed and expired
// environments release theirs; stopped ones keep theirs so a start
// resumes on the same mapping.
pub async fn next_electrum_port(
    pool: &PgPool,
    base: i32,
    max: i32,
) -> Result<Option<i32>, DbError> {
    let held: Vec<i32> = sqlx::query_scalar(
        "select electrum_port from environments \
         where electrum_port is not null and status in ('provisioning', 'ready', 'stopped')",
    )
    .fetch_all(pool)
    .await
    .map_err(DbError::from)?;
    Ok(lowest_free_port(base, max, &held))
}

/// The lowest port in `[base, max]` absent from `held`, if any.
fn lowest_free_port(base: i32, max: i32, held: &[i32]) -> Option<i32> {
    if base > max {
        return None;
    }
    let mut taken = held.to_vec();
    taken.sort_unstable();
    taken.dedup();
    let mut candidate = base;
    for port in taken {
        if port < candidate {
            continue;
        }
        if port > candidate {
            break;
        }
        candidate += 1;
    }
    (candidate <= max).then_some(candidate)
}

/// Remove one environment's record. Only records whose cluster state is
/// already gone (destroyed or expired) qualify; the status guard inside
/// the statement keeps a live environment safe under a stale caller.
pub async fn purge_environment(pool: &PgPool, id: Uuid) -> Result<bool, DbError> {
    let result = sqlx::query(
        "delete from environments where id = $1 and status in ('destroyed', 'expired')",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(DbError::from)?;
    Ok(result.rows_affected() > 0)
}

#[derive(FromRow, Debug, Clone)]
pub struct ApiTokenRow {
    pub npub_owner: String,
    pub revoked: bool,
}

#[cfg(test)]
mod tests {
    use super::{free_port_pair, lowest_free_port};

    #[test]
    fn empty_pool_starts_at_base() {
        assert_eq!(lowest_free_port(50002, 50011, &[]), Some(50002));
    }

    #[test]
    fn sequential_fill_returns_next_port() {
        assert_eq!(lowest_free_port(50002, 50011, &[50002, 50003]), Some(50004));
    }

    #[test]
    fn freed_low_port_is_reused() {
        // The regression: max()+1 never gave a released port back, so
        // destroyed environments permanently consumed the range.
        assert_eq!(lowest_free_port(50002, 50011, &[50003, 50004]), Some(50002));
    }

    #[test]
    fn middle_gap_is_filled_first() {
        assert_eq!(lowest_free_port(50002, 50011, &[50002, 50004]), Some(50003));
    }

    #[test]
    fn exhausted_range_is_none() {
        assert_eq!(lowest_free_port(50002, 50004, &[50002, 50003, 50004]), None);
    }

    #[test]
    fn single_port_range() {
        assert_eq!(lowest_free_port(7, 7, &[]), Some(7));
        assert_eq!(lowest_free_port(7, 7, &[7]), None);
    }

    #[test]
    fn pairs_start_at_base_when_empty() {
        assert_eq!(free_port_pair(21000, 22199, &[]), Some(21000));
    }

    #[test]
    fn pairs_are_even_aligned_and_contiguous() {
        // 21000 taken: the next pair starts at 21002, never straddling.
        assert_eq!(free_port_pair(21000, 22199, &[21000, 21001]), Some(21002));
    }

    #[test]
    fn freed_pairs_are_reused() {
        assert_eq!(free_port_pair(21000, 22199, &[21002, 21003]), Some(21000));
    }

    #[test]
    fn a_straddling_port_blocks_both_pairs() {
        // 21001 held: pair (21000, 21001) collides, pair (21002, 21003) fits.
        assert_eq!(free_port_pair(21000, 22199, &[21001]), Some(21002));
    }

    #[test]
    fn exhausted_pair_range_is_none() {
        assert_eq!(
            free_port_pair(21000, 21003, &[21000, 21001, 21002, 21003]),
            None
        );
    }
}

pub async fn insert_api_token(
    pool: &PgPool,
    token_hash: &str,
    npub_owner: &str,
) -> Result<(), DbError> {
    sqlx::query("insert into api_tokens (id, npub_owner, token_hash) values ($1, $2, $3)")
        .bind(Uuid::now_v7())
        .bind(npub_owner)
        .bind(token_hash)
        .execute(pool)
        .await
        .map_err(DbError::from)?;
    Ok(())
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

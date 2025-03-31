use sqlx::{postgres::PgPoolOptions, PgPool, migrate::Migrator};
use anyhow::Result;

//static MIGRATOR: Migrator = sqlx::migrate!();

pub type Db = PgPool;

pub async fn initDb(connection_string: &str) -> Result<Db> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&connection_string)
        .await?;

    //MIGRATOR.run(&pool).await?;

    Ok(pool)
}

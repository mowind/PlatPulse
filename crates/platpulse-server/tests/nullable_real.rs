//! Regression coverage for binding a nullable real through the backend-neutral
//! driver.
//!
//! A statement is executed twice on the same connection: once without a sample
//! and then with one. PostgreSQL caches the prepared statement together with
//! the parameter types it inferred on the first execution, so binding
//! `Option<f64>` directly makes the second execution fail with
//! `22P03 incorrect binary data format in bind parameter 1`.
//! `database::NullableReal` keeps the parameter `float8` on both paths
//! (ADR 0013).

use platpulse_server::database::NullableReal;

#[tokio::test]
async fn a_nullable_real_survives_a_missing_then_present_sample() {
    let (_dir, database) = platpulse_server::test_support::test_database().await;
    let pool = database.pool();
    sqlx::query("CREATE TABLE nullable_real_probe (value DOUBLE PRECISION)")
        .execute(pool)
        .await
        .expect("create the probe table");

    sqlx::query("INSERT INTO nullable_real_probe (value) VALUES ($1)")
        .bind(NullableReal(None))
        .execute(pool)
        .await
        .expect("bind a missing sample");
    sqlx::query("INSERT INTO nullable_real_probe (value) VALUES ($1)")
        .bind(NullableReal(Some(12.5)))
        .execute(pool)
        .await
        .expect("bind a present sample after a missing one");

    let values: Vec<Option<f64>> =
        sqlx::query_scalar("SELECT value FROM nullable_real_probe ORDER BY value NULLS FIRST")
            .fetch_all(pool)
            .await
            .expect("read the probe rows");
    assert_eq!(values, vec![None, Some(12.5)]);

    database.close().await;
}

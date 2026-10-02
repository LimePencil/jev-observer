//! Local stress-harness helper. Holds a writer lock on an encrypted workspace.
use std::io;

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .context("Database path is required")?;
    let key = std::env::var("JEV_OBSERVER_DB_KEY").context("Database key is required")?;
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Database key must be 64 hexadecimal characters");
    }
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "key", format!("x'{key}'"))?;
    connection.query_row("SELECT count(*) FROM sqlite_schema", [], |row| {
        row.get::<_, i64>(0)
    })?;
    connection.execute_batch("BEGIN IMMEDIATE")?;
    println!("locked");
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    connection.execute_batch("ROLLBACK")?;
    Ok(())
}

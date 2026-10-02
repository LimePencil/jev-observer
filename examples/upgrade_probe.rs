//! Local upgrade-fixture access to SQLCipher; never reads provider credentials.
use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use serde_json::json;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if !(3..=4).contains(&args.len()) {
        bail!(
            "Usage: upgrade_probe DATABASE seed-approval|show|future-schema|future-plaintext|seed-history|age-history [COUNT]"
        );
    }
    let mode = args[2].as_str();
    let mut connection = Connection::open(&args[1]).context("Open upgrade fixture")?;
    if mode != "future-plaintext" {
        let key = std::env::var("JEV_OBSERVER_DB_KEY").context("Database key is required")?;
        if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("Database key must contain 64 hexadecimal characters");
        }
        connection.pragma_update(None, "key", format!("x'{key}'"))?;
    }
    match mode {
        "seed-approval" => {
            connection.execute(
                "INSERT OR REPLACE INTO credential_approval(id,token_hash) VALUES(1,?)",
                ["0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"],
            )?;
        }
        "show" => {
            let approval: Option<String> = connection.query_row(
                "SELECT (SELECT token_hash FROM credential_approval WHERE id=1)",
                [],
                |row| row.get(0),
            )?;
            let requests: i64 =
                connection.query_row("SELECT count(*) FROM requests", [], |row| row.get(0))?;
            println!("{}", json!({"approval": approval, "requests": requests}));
        }
        "future-schema" | "future-plaintext" => {
            connection.pragma_update(None, "journal_mode", "DELETE")?;
            connection.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL); \
                 DELETE FROM schema_version; INSERT INTO schema_version VALUES(999);",
            )?;
        }
        "seed-history" => {
            let count: i64 = args.get(3).context("Fixture count is required")?.parse()?;
            if !(2..=200_000).contains(&count) {
                bail!("Fixture count must be between 2 and 200000");
            }
            let existing: i64 =
                connection.query_row("SELECT count(*) FROM requests", [], |row| row.get(0))?;
            if existing != 1 {
                bail!("Seed history requires exactly one captured template request");
            }
            let transaction = connection.transaction()?;
            transaction
                .execute_batch("CREATE TEMP TABLE fixture_numbers(n INTEGER PRIMARY KEY);")?;
            transaction.execute(
                "INSERT INTO fixture_numbers WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<?) SELECT n FROM numbers",
                [count - 1],
            )?;
            transaction.execute_batch(
                "INSERT INTO requests(id,timestamp,source,model,status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,capture_complete,sample,source_event_id,import_format,event_kind,data) \
                 SELECT 'history-'||n,r.timestamp-n*1000,r.source,r.model,r.status,r.duration_ms,r.input_tokens,r.output_tokens,r.cost_usd,r.cost_basis,r.answer_count,r.capture_complete,r.sample,NULL,NULL,r.event_kind,json_set(r.data,'$.id','history-'||n,'$.timestamp',r.timestamp-n*1000) FROM fixture_numbers CROSS JOIN requests r WHERE r.seq=1; \
                 INSERT INTO answers(request_id,key,group_id,kind,valid,value_num,value_text,confidence,data) \
                 SELECT 'history-'||n,a.key,a.group_id,a.kind,a.valid,a.value_num,a.value_text,a.confidence,a.data FROM fixture_numbers CROSS JOIN answers a WHERE a.request_id=(SELECT id FROM requests WHERE seq=1);",
            )?;
            transaction.commit()?;
        }
        "age-history" => {
            let old = chrono::Utc::now().timestamp_millis() - 8 * 86_400_000;
            connection.execute(
                "UPDATE requests SET timestamp=?,data=json_set(data,'$.timestamp',?) WHERE seq<=(SELECT count(*)/10 FROM requests)",
                [old, old],
            )?;
        }
        _ => bail!("Unknown upgrade fixture operation"),
    }
    Ok(())
}

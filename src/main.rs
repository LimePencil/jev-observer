mod access;
mod catalog;
mod collector;
mod config;
mod credentials;
mod model;
mod server;
mod store;
#[cfg(windows)]
mod windows_private;

use anyhow::{Context, Result};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let mut config = config::Config::parse();
    config.validate()?;
    let path = config.database_path();
    let retention_days = config.retention_days;
    let max_records = config.max_records;
    let demo = config.demo;
    let database_key = if demo {
        None
    } else {
        Some(store::Store::key_from_environment()?)
    };
    let store = tokio::task::spawn_blocking(move || -> Result<store::Store> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).context("Create database directory")?;
        }
        let store = if let Some(key) = database_key {
            store::Store::open_encrypted(&path, key, retention_days, max_records)?
        } else {
            store::Store::open_demo(&path, retention_days, max_records)?
        };
        if demo {
            seed_demo_if_empty(&store)?;
        }
        Ok(store)
    })
    .await
    .context("Initialize database worker")??;
    let collector = collector::Collector::start(
        store.clone(),
        config.normalize_options(),
        config.capture_limit,
        config.capture_slots,
        config.queue_capacity,
    );
    server::run(config, store, collector).await
}

fn seed_demo_if_empty(store: &store::Store) -> Result<()> {
    // Any retained event, including an imported application action, makes this
    // an existing workspace. Startup needs only an existence check.
    if store.is_empty()? {
        let mut connection = store.writer_connection()?;
        store.write_batch(&mut connection, &model::sample_records())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn demo_startup_preserves_action_only_history() {
        let directory = tempfile::tempdir().unwrap();
        let store = store::Store::open(&directory.path().join("demo.sqlite"), 7, 1000).unwrap();
        let mut action = model::sample_records().remove(0);
        action["id"] = json!("existing-action");
        action["event_kind"] = json!("application_action");
        action["answers"] = json!([]);
        action["labels"] = json!([]);
        action["input_tokens"] = Value::Null;
        action["output_tokens"] = Value::Null;
        action["cost_usd"] = Value::Null;
        action["cost_basis"] = Value::Null;
        store
            .write_batch(&mut store.writer_connection().unwrap(), &[action])
            .unwrap();
        let filter = store::Filter {
            window: Some("all".into()),
            ..Default::default()
        };
        let before = store.export(&filter, "jsonl").unwrap();
        seed_demo_if_empty(&store).unwrap();
        let after = store.export(&filter, "jsonl").unwrap();
        assert_eq!(after.lines().count(), 1);
        assert_eq!(after, before);
    }

    #[test]
    fn demo_startup_seeds_empty_history_once() {
        let directory = tempfile::tempdir().unwrap();
        let store = store::Store::open(&directory.path().join("demo.sqlite"), 7, 1000).unwrap();
        assert!(store.is_empty().unwrap());
        seed_demo_if_empty(&store).unwrap();
        assert!(!store.is_empty().unwrap());
        let filter = store::Filter {
            window: Some("all".into()),
            ..Default::default()
        };
        let seeded = store.export(&filter, "jsonl").unwrap();
        seed_demo_if_empty(&store).unwrap();
        assert_eq!(store.export(&filter, "jsonl").unwrap(), seeded);
    }
}

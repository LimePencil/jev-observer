use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::{
    access::Access, collector::Collector, config::Config, credentials::Credentials, store::Store,
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub store: Store,
    pub collector: Collector,
    pub client: reqwest::Client,
    pub shutdown: CancellationToken,
    pub credentials: Credentials,
    pub access: Option<Access>,
    pub(super) database_slots: Arc<Semaphore>,
    pub(super) import_slots: Arc<Semaphore>,
    pub(super) export_slots: Arc<Semaphore>,
}

impl AppState {
    pub fn new(config: Config, store: Store, collector: Collector) -> Result<Self> {
        #[cfg(not(test))]
        let credentials = Credentials::open(&config.database_path(), store.clone())?;
        #[cfg(test)]
        let credentials = Credentials::empty();
        #[cfg(not(test))]
        let (access, access_path) = Access::open(&config.database_path())?;
        #[cfg(not(test))]
        let access = Some(access);
        #[cfg(test)]
        let access: Option<Access> = None;
        #[cfg(not(test))]
        eprintln!("Dashboard access token: {}", access_path.display());
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(15))
            // Non-streaming local models can legitimately spend minutes on
            // inference before sending headers. Use one configurable total
            // deadline instead of silently imposing a shorter read deadline.
            .timeout(Duration::from_secs(config.upstream_timeout_secs))
            .tcp_nodelay(true)
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(64)
            .build()
            .context("Create upstream HTTP client")?;
        Ok(Self {
            config: Arc::new(config),
            store,
            collector,
            client,
            shutdown: CancellationToken::new(),
            credentials,
            access,
            database_slots: Arc::new(Semaphore::new(8)),
            import_slots: Arc::new(Semaphore::new(1)),
            export_slots: Arc::new(Semaphore::new(1)),
        })
    }
}

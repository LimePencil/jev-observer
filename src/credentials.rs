use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::store::Store;

const SERVICE: &str = "jev-observer/provider-connection";
pub const LOCAL_TOKEN_PREFIX: &str = "jo_local_";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    api_key: String,
    client_token: String,
}

#[derive(Default)]
struct Active {
    pair: Option<Pair>,
    persisted: bool,
}

#[derive(Clone)]
pub struct Credentials {
    active: Arc<RwLock<Active>>,
    account: String,
    mutation: Arc<Semaphore>,
    approval_store: Option<Store>,
}

#[derive(Serialize)]
pub struct Status {
    pub configured: bool,
    pub storage: &'static str,
}

#[cfg(not(test))]
fn account_for(path: &std::path::Path) -> Result<String> {
    let path = path
        .canonicalize()
        .context("Resolve workspace database path")?;
    let hash = Sha256::digest(path.to_string_lossy().as_bytes());
    Ok(format!("{hash:x}"))
}

fn entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).context("Open system credential store")
}

#[cfg(not(test))]
fn load(account: &str) -> Result<Option<Pair>> {
    match entry(account)?.get_password() {
        Ok(text) => {
            let pair: Pair =
                serde_json::from_str(&text).context("Decode saved provider connection")?;
            validate_key(&pair.api_key)?;
            if !valid_local_token(&pair.client_token) {
                bail!("Saved local client token is invalid");
            }
            Ok(Some(pair))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error).context("Read system credential store"),
    }
}

fn save(account: &str, pair: &Pair) -> Result<()> {
    let text = serde_json::to_string(pair)?;
    entry(account)?
        .set_password(&text)
        .context("Save provider connection")
}

fn forget(account: &str) -> Result<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error).context("Remove saved provider connection"),
    }
}

pub(crate) fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 4096 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        bail!("Provider API key must be 1–4096 printable ASCII characters without spaces");
    }
    Ok(())
}

fn valid_local_token(token: &str) -> bool {
    token.starts_with(LOCAL_TOKEN_PREFIX)
        && token.len() == LOCAL_TOKEN_PREFIX.len() + 64
        && token[LOCAL_TOKEN_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn same_token(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

impl Credentials {
    #[cfg(not(test))]
    pub fn open(path: &std::path::Path, store: Store) -> Result<Self> {
        let account = account_for(path)?;
        let approval = store.credential_approval()?;
        let active = match approval
            .as_deref()
            .map(|approved| (approved, load(&account)))
        {
            Some((approved, Ok(Some(pair)))) if token_hash(&pair.client_token) == approved => {
                Active {
                    pair: Some(pair),
                    persisted: true,
                }
            }
            Some((_, result)) => {
                eprintln!(
                    "Observer could not activate its approved system credential entry: {}",
                    if result.is_err() {
                        "credential store unavailable"
                    } else {
                        "entry missing or changed"
                    }
                );
                Active::default()
            }
            None => Active::default(),
        };
        Ok(Self {
            active: Arc::new(RwLock::new(active)),
            account,
            mutation: Arc::new(Semaphore::new(1)),
            approval_store: Some(store),
        })
    }

    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            active: Arc::new(RwLock::new(Active::default())),
            account: "test-only".into(),
            mutation: Arc::new(Semaphore::new(1)),
            approval_store: None,
        }
    }

    pub fn status(&self) -> Status {
        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Status {
            configured: active.pair.is_some(),
            storage: if active.pair.is_none() {
                "none"
            } else if active.persisted {
                "system"
            } else {
                "session"
            },
        }
    }

    pub fn provider_for_token(&self, token: &str) -> Option<String> {
        let active = self
            .active
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        active
            .pair
            .as_ref()
            .and_then(|pair| same_token(&pair.client_token, token).then(|| pair.api_key.clone()))
    }

    pub async fn set(&self, api_key: String, persist: bool) -> Result<String> {
        validate_key(&api_key)?;
        let permit = self
            .mutation
            .clone()
            .acquire_owned()
            .await
            .context("Credential operation closed")?;
        let token = format!(
            "{LOCAL_TOKEN_PREFIX}{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let pair = Pair {
            api_key,
            client_token: token.clone(),
        };
        let active = self.active.clone();
        let account = self.account.clone();
        let approval_store = self.approval_store.clone();
        // Keep the serialized credential operation alive if its HTTP caller leaves.
        tokio::task::spawn_blocking(move || -> Result<String> {
            let _permit = permit;
            let was_persisted = active
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .persisted;
            let old_approval = approval_store
                .as_ref()
                .map(Store::credential_approval)
                .transpose()?
                .flatten();
            if persist {
                save(&account, &pair)?;
                if let Some(store) = &approval_store {
                    store.set_credential_approval(Some(&token_hash(&token)))?;
                }
            } else {
                if let Some(store) = &approval_store {
                    // Disapprove the old entry before attempting deletion. If
                    // the OS store is locked, it cannot revive on restart.
                    store.set_credential_approval(None)?;
                }
                if (was_persisted || old_approval.is_some()) && forget(&account).is_err() {
                    eprintln!("Observer could not remove the obsolete system credential entry");
                }
            }
            *active
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Active {
                pair: Some(pair),
                persisted: persist,
            };
            Ok(token)
        })
        .await
        .context("System credential operation failed")?
    }

    pub async fn clear(&self) -> Result<()> {
        let permit = self
            .mutation
            .clone()
            .acquire_owned()
            .await
            .context("Credential operation closed")?;
        let active = self.active.clone();
        let account = self.account.clone();
        let approval_store = self.approval_store.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let _permit = permit;
            let persisted = active
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .persisted;
            let old_approval = approval_store
                .as_ref()
                .map(Store::credential_approval)
                .transpose()?
                .flatten();
            if let Some(store) = &approval_store {
                store.set_credential_approval(None)?;
            }
            *active
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Active::default();
            if persisted || old_approval.is_some() {
                forget(&account)?;
            }
            Ok(())
        })
        .await
        .context("System credential operation failed")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_token_replaces_key_and_rotates_without_exposing_it_in_status() {
        let credentials = Credentials::empty();
        assert!(!credentials.status().configured);
        let first = credentials.set("provider-one".into(), false).await.unwrap();
        assert!(valid_local_token(&first));
        assert_eq!(
            credentials.provider_for_token(&first).as_deref(),
            Some("provider-one")
        );
        assert_eq!(credentials.status().storage, "session");
        let second = credentials.set("provider-two".into(), false).await.unwrap();
        assert_ne!(first, second);
        assert!(credentials.provider_for_token(&first).is_none());
        assert_eq!(
            credentials.provider_for_token(&second).as_deref(),
            Some("provider-two")
        );
        credentials.clear().await.unwrap();
        assert!(credentials.provider_for_token(&second).is_none());
    }
}

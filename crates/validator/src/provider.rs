//! Multi-RPC failover (mirrors this repo's `ChainProvider` + `Web3Service`).
//!
//! Holds an ordered list of RPC endpoints. Every call tries the currently
//! active endpoint first, then rotates through the rest on error, sticking to
//! the first one that answers. A `chainId` guard at startup drops endpoints
//! that report the wrong chain (the classic "pointed at the wrong network" bug).

use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::{Filter, Log};
use tracing::warn;

struct Endpoint {
    url: String,
    provider: DynProvider,
}

pub struct Failover {
    endpoints: Vec<Endpoint>,
    active: usize,
}

/// Connect to the first endpoint that answers AND reports `expected_chain_id`.
///
/// Used by the refund loop, which needs a plain provider for `eth_call` reads of
/// gate state rather than the log-scanning surface [`Failover`] exposes. The
/// chainId guard matters more here than anywhere: attesting a refund on the
/// strength of a *different* chain's `executed` flag would be exactly the
/// mistake that lets a delivered transfer also be refunded.
pub async fn connect_checked(urls: &[String], expected_chain_id: u64) -> anyhow::Result<DynProvider> {
    for url in urls {
        let provider = match url.parse() {
            Ok(parsed) => ProviderBuilder::new().connect_http(parsed).erased(),
            Err(e) => {
                warn!(%url, error = %e, "skipping unparseable RPC url");
                continue;
            }
        };
        match provider.get_chain_id().await {
            Ok(id) if id == expected_chain_id => return Ok(provider),
            Ok(id) => warn!(%url, got = id, want = expected_chain_id, "skipping RPC: chainId mismatch"),
            Err(e) => warn!(%url, error = %e, "skipping RPC: unreachable"),
        }
    }
    anyhow::bail!("no healthy RPC endpoints for chain {expected_chain_id}")
}

impl Failover {
    /// Connect to every URL, keep only those whose `eth_chainId` matches
    /// `expected_chain_id`. Errors if none survive.
    pub async fn connect(urls: &[String], expected_chain_id: u64) -> anyhow::Result<Self> {
        let mut endpoints = Vec::new();
        for url in urls {
            let provider = match url.parse() {
                Ok(parsed) => ProviderBuilder::new().connect_http(parsed).erased(),
                Err(e) => {
                    warn!(%url, error = %e, "skipping unparseable RPC url");
                    continue;
                }
            };
            match provider.get_chain_id().await {
                Ok(id) if id == expected_chain_id => {
                    endpoints.push(Endpoint { url: url.clone(), provider });
                }
                Ok(id) => warn!(%url, got = id, want = expected_chain_id, "skipping RPC: chainId mismatch"),
                Err(e) => warn!(%url, error = %e, "skipping RPC: unreachable"),
            }
        }
        anyhow::ensure!(
            !endpoints.is_empty(),
            "no healthy RPC endpoints for chain {expected_chain_id}"
        );
        Ok(Self { endpoints, active: 0 })
    }

    pub fn active_url(&self) -> &str {
        &self.endpoints[self.active].url
    }

    /// Run an async provider op across endpoints, rotating on failure and
    /// pinning `active` to the first that succeeds.
    async fn with_failover<T, F, Fut>(&mut self, what: &str, mut op: F) -> anyhow::Result<T>
    where
        F: FnMut(DynProvider) -> Fut,
        Fut: std::future::Future<Output = Result<T, alloy::transports::TransportError>>,
    {
        let n = self.endpoints.len();
        let mut last_err = None;
        for attempt in 0..n {
            let idx = (self.active + attempt) % n;
            let provider = self.endpoints[idx].provider.clone();
            match op(provider).await {
                Ok(v) => {
                    if idx != self.active {
                        warn!(from = %self.endpoints[self.active].url, to = %self.endpoints[idx].url, "RPC failover");
                        self.active = idx;
                    }
                    return Ok(v);
                }
                Err(e) => {
                    warn!(url = %self.endpoints[idx].url, op = what, error = %e, "RPC call failed; rotating");
                    last_err = Some(e);
                }
            }
        }
        Err(anyhow::anyhow!(
            "all {n} RPC endpoints failed for {what}: {}",
            last_err.map(|e| e.to_string()).unwrap_or_default()
        ))
    }

    /// A clone of the currently-active provider, for one-off contract reads that
    /// do not warrant a dedicated failover wrapper (e.g. the startup read of
    /// `Gate.bridgeDomain()`). Callers own the retry: this hands back whichever
    /// endpoint is active right now and does not rotate on failure.
    pub fn active_provider(&self) -> DynProvider {
        self.endpoints[self.active].provider.clone()
    }

    pub async fn get_block_number(&mut self) -> anyhow::Result<u64> {
        self.with_failover("get_block_number", |p| async move { p.get_block_number().await })
            .await
    }

    pub async fn get_logs(&mut self, filter: &Filter) -> anyhow::Result<Vec<Log>> {
        let filter = filter.clone();
        self.with_failover("get_logs", move |p| {
            let filter = filter.clone();
            async move { p.get_logs(&filter).await }
        })
        .await
    }
}

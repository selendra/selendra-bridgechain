//! graphql-api — a read-mostly GraphQL front-end over the bridge signature store.
//!
//! It answers the questions an operator/dapp actually asks ("what transfers are
//! in flight A->C?", "is this submissionId ready to claim?", "how many are stuck
//! below threshold?") in one typed query, instead of fetching every record from
//! the sig-store and filtering client-side. It reads the same store the
//! validator/keeper share (a dir or the HTTP sig-store) and exposes one mutation
//! (`submitSignature`) that goes through the existing trust boundary.
//!
//!   GET  /            -> GraphiQL playground (interactive explorer)
//!   POST /graphql     -> GraphQL endpoint
//!   GET  /health      -> "ok"
//!
//! Read-only by default for safety; pass `--allow-mutations` to expose the
//! `submitSignature` mutation. Bind to localhost unless you front it with auth.

mod backend;
mod chain;
mod schema;
mod swap;

use std::sync::Arc;

use async_graphql::http::GraphiQLSource;
use async_graphql::{EmptyMutation, Schema};
use async_graphql_axum::GraphQL;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::Router;
use clap::Parser;
use tracing::info;

use backend::Backend;
use chain::Chains;
use schema::{ApiState, Mutation, Query};
use swap::Swaps;

#[derive(Parser, Debug)]
#[command(about = "GraphQL API over the bridge signature store")]
struct Args {
    /// Address to bind, e.g. 127.0.0.1:8088
    #[arg(long, env = "GRAPHQL_BIND", default_value = "127.0.0.1:8088")]
    bind: String,
    /// Directory-backed store (file-per-id). Mutually exclusive with --store-url.
    #[arg(long, env = "GRAPHQL_STORE_DIR")]
    dir: Option<String>,
    /// HTTP sig-store base URL, e.g. http://127.0.0.1:8080.
    #[arg(long, env = "GRAPHQL_STORE_URL")]
    store_url: Option<String>,
    /// Keeper signature threshold, so the API can report `meetsThreshold`/`ready`.
    #[arg(long, env = "GRAPHQL_THRESHOLD")]
    threshold: Option<u64>,
    /// Destination gate(s) for on-chain `executed`/`status`, as `CHAINID=RPC,GATE`
    /// (repeatable), e.g. `--gate 1338=http://127.0.0.1:8546,0xGate...`. Without
    /// it, `executed` is null and `status` falls back to signatures only.
    #[arg(long = "gate", value_name = "CHAINID=RPC,GATE")]
    gates: Vec<String>,
    /// JSON file listing the network registry served to the UI via the `chains`
    /// query (an array of {chainId,name,rpcUrl?,gate?,token?}). Each chain with
    /// an rpcUrl+gate is also registered for `executed()` lookups (an explicit
    /// `--gate` for the same chain wins). Omit it => `chains` returns `[]`.
    #[arg(long = "chains-file", env = "GRAPHQL_CHAINS_FILE", value_name = "PATH")]
    chains_file: Option<String>,
    /// Same-chain SwapPool(s) for the `pools`/`swapQuote` read view, as
    /// `CHAINID=RPC,POOL` (repeatable), e.g.
    /// `--swap 1337=http://127.0.0.1:8545,0xPool...`. Without it, `pools` and
    /// `swapQuote` return null. Read-only — no swaps are executed server-side.
    #[arg(long = "swap", value_name = "CHAINID=RPC,POOL")]
    swaps: Vec<String>,
    /// Expose the `submitSignature` mutation (off by default — read-only).
    #[arg(long, env = "GRAPHQL_ALLOW_MUTATIONS", default_value_t = false)]
    allow_mutations: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "graphql_api=info".into()),
        )
        .init();

    let args = Args::parse();

    let backend = match (&args.dir, &args.store_url) {
        (Some(_), Some(_)) => anyhow::bail!("pass only one of --dir or --store-url"),
        (Some(dir), None) => Backend::file(dir),
        (None, Some(url)) => Backend::remote(url.clone()),
        (None, None) => anyhow::bail!("need a store: pass --dir <path> or --store-url <url>"),
    };
    let described = backend.describe();

    let mut chains = Chains::new();
    for spec in &args.gates {
        chains.add_spec(spec)?;
    }

    // Load the UI-facing registry (if any) and fold each chain's gate into the
    // executed-gate map, so one --chains-file can drive both the `chains` query
    // and on-chain status without repeating every gate as a --gate flag.
    let registry = match &args.chains_file {
        Some(path) => chain::load_registry(path)?,
        None => Vec::new(),
    };
    for c in &registry {
        if let (Some(rpc), Some(gate)) = (&c.rpc_url, &c.gate) {
            chains.add(c.chain_id, rpc, gate)?;
        }
    }
    let chain_ids = chains.configured();

    let mut swaps = Swaps::new();
    for spec in &args.swaps {
        swaps.add_spec(spec)?;
    }
    let swap_ids = swaps.configured();

    let state = ApiState {
        backend: Arc::new(backend),
        threshold: args.threshold,
        chains,
        registry,
        swaps,
    };

    // Depth/complexity caps so one request can't fan out to hundreds of store
    // loads or destination-gate RPCs (e.g. an alias bomb). Generous enough for
    // legit queries and GraphiQL's introspection, tight enough to stop abuse.
    let router = if args.allow_mutations {
        let schema = Schema::build(Query, Mutation, async_graphql::EmptySubscription)
            .limit_depth(15)
            .limit_complexity(1000)
            .data(state)
            .finish();
        Router::new().route("/graphql", get(graphiql).post_service(GraphQL::new(schema)))
    } else {
        let schema = Schema::build(Query, EmptyMutation, async_graphql::EmptySubscription)
            .limit_depth(15)
            .limit_complexity(1000)
            .data(state)
            .finish();
        Router::new().route("/graphql", get(graphiql).post_service(GraphQL::new(schema)))
    };

    let app = router
        .route("/", get(graphiql))
        .route("/health", get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    info!(
        bind = %args.bind,
        store = %described,
        threshold = ?args.threshold,
        mutations = args.allow_mutations,
        on_chain_status_for = ?chain_ids,
        swap_pools_for = ?swap_ids,
        "graphql-api listening (GraphiQL at /)"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// The interactive GraphiQL explorer, pointed at our /graphql endpoint.
async fn graphiql() -> impl IntoResponse {
    Html(GraphiQLSource::build().endpoint("/graphql").finish())
}

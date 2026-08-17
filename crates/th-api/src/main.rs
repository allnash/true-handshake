//! True Handshake API server.
//!
//! Wires the concrete adapters to the ports once, at startup, and runs the timer
//! worker in the same process. Splitting the worker into its own binary is a
//! two-line change — the scheduler is already a library with no shared state
//! beyond the pool — but one process is the right shape for local development.

mod routes;
mod views;

use std::sync::Arc;

use th_app::{Handshake, Signer as _, SystemClock};
use th_infra::{
    ClaudeWitness, Ed25519Signer, MockEscrow, OfflineWitness, PgAudioStore, PgDealRepo,
    PgProofStore, PgSessionRepo, PgTaskQueue,
};
use th_jobs::Scheduler;
use tower_http::{cors::CorsLayer, limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing::{info, warn};

/// Handoff photos are the only large payload; 12 MB covers several phone images
/// with room to spare and keeps a runaway upload from becoming an outage.
const MAX_BODY_BYTES: usize = 12 * 1024 * 1024;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if there is one. Real environment variables always win, so a
    // deployment that sets them properly is unaffected by a stray file.
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "th_api=info,th_app=info,th_infra=info,th_jobs=info".into()),
        )
        .init();

    // `--generate-seed` prints a signing key and exits, so operators never have
    // to work out the encoding by hand.
    if std::env::args().any(|a| a == "--generate-seed") {
        println!("TH_SIGNING_SEED={}", Ed25519Signer::generate_seed_b64());
        return Ok(());
    }

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://truehandshake:truehandshake@localhost:5433/truehandshake".into());
    let bind = std::env::var("TH_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let public_base_url =
        std::env::var("TH_PUBLIC_BASE_URL").unwrap_or_else(|_| "http://localhost:5173".into());
    let currency = std::env::var("TH_CURRENCY").unwrap_or_else(|_| "USD".into());

    let pool = th_infra::connect(&database_url, 10).await?;
    th_infra::migrate(&pool).await?;
    info!("database ready");

    let signer = Arc::new(Ed25519Signer::from_seed_b64(
        std::env::var("TH_SIGNING_SEED").ok().as_deref(),
    )?);
    info!(key_id = %signer.key_id(), "signing key loaded");

    // With no API key the app still runs end to end on the offline witness. That
    // keeps a fresh clone runnable, and makes it obvious in the logs which
    // witness is actually reading people's conversations.
    let (witness, vision): (Arc<dyn th_app::Witness>, Arc<dyn th_app::VisionWitness>) =
        match std::env::var("ANTHROPIC_API_KEY") {
            Ok(key) if !key.trim().is_empty() => {
                info!("witness: Claude (claude-opus-5)");
                let c = Arc::new(ClaudeWitness::new(key));
                (c.clone(), c)
            }
            _ => {
                warn!(
                    "ANTHROPIC_API_KEY is not set — falling back to the offline witness, which \
                     only scans for numbers and cannot understand a conversation."
                );
                let w = Arc::new(OfflineWitness);
                (w.clone(), w)
            }
        };

    let clock = Arc::new(SystemClock);
    let tasks = Arc::new(PgTaskQueue::new(pool.clone()));

    let handshake = Arc::new(Handshake {
        clock: clock.clone(),
        deals: Arc::new(PgDealRepo::new(pool.clone())),
        sessions: Arc::new(PgSessionRepo::new(pool.clone())),
        witness,
        vision,
        proofs: Arc::new(PgProofStore::new(pool.clone())),
        audio: Arc::new(PgAudioStore::new(pool.clone())),
        settlement: Arc::new(MockEscrow::new(pool.clone())),
        signer: signer.clone(),
        tasks: tasks.clone(),
        default_currency: currency,
    });

    warn!(
        provider = handshake.settlement.id(),
        "settlement is a mock ledger: no real funds move, and evidence tier stays `attested`"
    );

    // The scheduler is what makes the 24-hour hold real rather than decorative.
    let scheduler = Scheduler::new(handshake.clone(), tasks, clock);
    tokio::spawn(scheduler.run());

    let state = routes::AppState {
        handshake,
        public_base_url,
        mediator_token: std::env::var("TH_MEDIATOR_TOKEN").ok().filter(|t| !t.is_empty()),
    };

    let app = routes::router(state)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        // The SPA is served from a different origin in development; in
        // production it is a static bundle on a CDN talking to this same public
        // API, with no privileged back door of its own.
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    info!(%bind, "true handshake api listening");
    axum::serve(listener, app).await?;
    Ok(())
}

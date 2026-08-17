//! The timer worker.
//!
//! Deadlines are the product, so they are not a cron job scanning tables. Each
//! deadline is a row in `scheduled_tasks` written in the same transaction as the
//! transition that created it, claimed with `FOR UPDATE SKIP LOCKED` so several
//! replicas can drain the queue safely.
//!
//! Firing a timer is not a privileged path: it invokes the same use case a human
//! would, with a `System` actor, and the domain decides whether that is still
//! legal. A worker that resumes six hours late therefore does the right thing —
//! it either applies a transition that is still valid or finds it moot, and it
//! can never reach into a deal and set a state directly.

use std::sync::Arc;
use std::time::Duration;

use th_app::{Clock, Handshake, TaskQueue};
use tracing::{error, info, warn};

pub struct Scheduler {
    pub handshake: Arc<Handshake>,
    pub tasks: Arc<dyn TaskQueue>,
    pub clock: Arc<dyn Clock>,
    /// How often to look for work when the queue is empty.
    pub poll_interval: Duration,
    pub batch_size: i64,
}

impl Scheduler {
    pub fn new(handshake: Arc<Handshake>, tasks: Arc<dyn TaskQueue>, clock: Arc<dyn Clock>) -> Self {
        Self {
            handshake,
            tasks,
            clock,
            poll_interval: Duration::from_secs(5),
            batch_size: 32,
        }
    }

    /// Run until the process is cancelled.
    pub async fn run(self) {
        info!(
            poll_interval_secs = self.poll_interval.as_secs(),
            "timer worker started"
        );
        loop {
            match self.tick().await {
                Ok(0) => tokio::time::sleep(self.poll_interval).await,
                // Work was found; look again immediately in case there is more.
                Ok(_) => {}
                Err(e) => {
                    error!(error = %e, "timer sweep failed; backing off");
                    tokio::time::sleep(self.poll_interval).await;
                }
            }
        }
    }

    /// One sweep. Returns how many tasks were claimed.
    pub async fn tick(&self) -> Result<usize, th_app::AppError> {
        let now = self.clock.now();
        let due = self.tasks.claim_due(now, self.batch_size).await?;
        let claimed = due.len();

        for task in due {
            let lateness = now - task.due_at;
            if lateness > time::Duration::minutes(2) {
                // Visible rather than silent: a late fire is safe, but a
                // systematically late one is an operational problem.
                warn!(
                    deal_id = %task.deal_id,
                    kind = task.kind.as_str(),
                    late_seconds = lateness.whole_seconds(),
                    "timer fired late"
                );
            }

            match self.handshake.fire_task(&task).await {
                Ok(Some(result)) => {
                    info!(
                        deal_id = %task.deal_id,
                        kind = task.kind.as_str(),
                        to = result.deal.state.as_str(),
                        "timer applied"
                    );
                    self.tasks.complete(task.id).await?;
                }
                // The deal moved on before the timer fired. Not an error.
                Ok(None) => {
                    self.tasks.complete(task.id).await?;
                }
                Err(e) => {
                    error!(deal_id = %task.deal_id, error = %e, "timer failed; will retry");
                    self.tasks.fail(task.id, &e.to_string()).await?;
                }
            }
        }

        Ok(claimed)
    }
}

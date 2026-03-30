use std::time::{Duration, Instant};

pub(super) struct BatchRefreshScheduler {
    interval: Duration,
    pub(super) last_run: Option<Instant>,
}

impl BatchRefreshScheduler {
    pub(super) fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_run: None,
        }
    }

    pub(super) fn should_trigger(&self) -> bool {
        match self.last_run {
            None => true,
            Some(instant) => instant.elapsed() >= self.interval,
        }
    }

    pub(super) fn mark_triggered(&mut self) {
        self.last_run = Some(Instant::now());
    }
}

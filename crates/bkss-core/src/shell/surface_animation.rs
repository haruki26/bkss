use crate::{AnimationPlan, AnimationRunner, AnimationSample, AnimationToken, Phase, RunnerError};
use std::time::{Duration, Instant};

pub(super) struct SurfaceAnimation {
    runner: AnimationRunner,
    phase: Phase,
    token: AnimationToken,
    sample: AnimationSample,
    frame_pending: bool,
    epoch: Instant,
}

impl SurfaceAnimation {
    pub(super) fn new(plan: AnimationPlan, epoch: Instant) -> Result<Self, RunnerError> {
        let mut runner = AnimationRunner::new(plan)?;
        let token = runner.request(Phase::Enter, Duration::ZERO)?;
        let sample = runner.sample(Duration::ZERO)?;
        Ok(Self {
            runner,
            phase: Phase::Enter,
            token,
            sample,
            frame_pending: false,
            epoch,
        })
    }

    pub(super) fn request(&mut self, phase: Phase, now: Instant) -> Result<(), RunnerError> {
        let elapsed = now.saturating_duration_since(self.epoch);
        self.token = self.runner.request(phase, elapsed)?;
        self.phase = phase;
        self.sample = self.runner.sample(elapsed)?;
        Ok(())
    }

    pub(super) fn sample(&mut self, now: Instant) -> Result<AnimationSample, RunnerError> {
        self.sample = self
            .runner
            .sample(now.saturating_duration_since(self.epoch))?;
        Ok(self.sample)
    }

    pub(super) fn value(&self) -> AnimationSample {
        self.sample
    }

    pub(super) fn phase(&self) -> Phase {
        self.phase
    }

    pub(super) fn token(&self) -> AnimationToken {
        self.token
    }

    pub(super) fn is_current(&self, token: AnimationToken) -> bool {
        self.runner.is_current(token)
    }

    pub(super) fn is_frame_pending(&self) -> bool {
        self.frame_pending
    }

    pub(super) fn set_frame_pending(&mut self, pending: bool) {
        self.frame_pending = pending;
    }
}

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

    pub(super) fn is_exit_completion(&self, token: AnimationToken) -> bool {
        self.phase == Phase::Exit && token == self.token && self.runner.is_current(token)
    }

    pub(super) fn is_frame_pending(&self) -> bool {
        self.frame_pending
    }

    pub(super) fn set_frame_pending(&mut self, pending: bool) {
        self.frame_pending = pending;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Channel, EasingCurve, PlanTiming, PropertyAnimation};

    fn plan() -> AnimationPlan {
        let timing = Some(PlanTiming::Easing {
            duration: Duration::from_millis(100),
            curve: EasingCurve::Linear,
        });
        AnimationPlan {
            opacity: Some(Channel {
                enter: Some(PropertyAnimation {
                    endpoints: [0.0, 1.0],
                    timing,
                }),
                exit: Some(PropertyAnimation {
                    endpoints: [1.0, 0.0],
                    timing,
                }),
            }),
            ..AnimationPlan::default()
        }
    }

    #[test]
    fn accepts_current_exit_completion() {
        let epoch = Instant::now();
        let mut animation = SurfaceAnimation::new(plan(), epoch).unwrap();
        animation
            .sample(epoch + Duration::from_millis(100))
            .unwrap();
        animation
            .request(Phase::Exit, epoch + Duration::from_millis(100))
            .unwrap();
        let completion = animation
            .sample(epoch + Duration::from_millis(200))
            .unwrap()
            .completion;

        assert_eq!(completion, Some(animation.token));
        assert!(completion.is_some_and(|token| animation.is_exit_completion(token)));
    }

    #[test]
    fn rejects_enter_completion() {
        let epoch = Instant::now();
        let mut animation = SurfaceAnimation::new(plan(), epoch).unwrap();
        let completion = animation
            .sample(epoch + Duration::from_millis(100))
            .unwrap()
            .completion;

        assert!(completion.is_some());
        assert!(completion.is_some_and(|token| !animation.is_exit_completion(token)));
    }

    #[test]
    fn rejects_exit_completion_after_retarget() {
        let epoch = Instant::now();
        let mut animation = SurfaceAnimation::new(plan(), epoch).unwrap();
        animation.request(Phase::Exit, epoch).unwrap();
        let exit_token = animation.token;
        animation
            .request(Phase::Enter, epoch + Duration::from_millis(1))
            .unwrap();

        assert!(!animation.is_exit_completion(exit_token));
    }
}

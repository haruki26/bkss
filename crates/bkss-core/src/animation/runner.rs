use std::time::Duration;

use super::plan::validate_plan;
use super::settlement::finish_elapsed;
use super::spring::spring_state;
use super::{
    AnimationPlan, AnimationSample, AnimationToken, Channel, EasingCurve, Phase, PlanTiming,
    PropertyAnimation, RunnerError,
};

#[derive(Clone, Copy)]
struct Motion {
    origin: (f64, f64),
    target: f64,
    span: f64,
    start: Duration,
    timing: Option<PlanTiming>,
    finish: Duration,
}

impl Motion {
    fn evaluate(&self, now: Duration) -> (f64, f64, bool) {
        let elapsed = now.saturating_sub(self.start);
        if elapsed >= self.finish {
            return (self.target, 0.0, false);
        }
        let Some(timing) = self.timing else {
            return (self.target, 0.0, false);
        };
        let seconds = elapsed.as_secs_f64();
        let (position, velocity) = match timing {
            PlanTiming::Easing { duration, curve } => {
                let duration = duration.as_secs_f64();
                if duration == 0.0 || self.origin.0 == self.target {
                    return (self.target, 0.0, false);
                }
                let t = (seconds / duration).clamp(0.0, 1.0);
                let (progress, derivative) = match curve {
                    EasingCurve::Linear => (t, 1.0),
                    EasingCurve::EaseIn => (t * t, 2.0 * t),
                    EasingCurve::EaseOut => (1.0 - (1.0 - t).powi(2), 2.0 * (1.0 - t)),
                    EasingCurve::EaseInOut if t < 0.5 => (2.0 * t * t, 4.0 * t),
                    EasingCurve::EaseInOut => {
                        (1.0 - (-2.0 * t + 2.0).powi(2) / 2.0, 4.0 * (1.0 - t))
                    }
                };
                let distance = self.target - self.origin.0;
                (
                    self.origin.0 + distance * progress,
                    distance * derivative / duration,
                )
            }
            PlanTiming::Spring {
                stiffness,
                damping,
                mass,
                ..
            } => spring_state(
                [stiffness as f64, damping as f64, mass as f64],
                self.origin.0,
                self.origin.1,
                self.target,
                seconds,
            ),
        };
        (position, velocity, true)
    }
}

#[derive(Clone, Copy)]
struct Active {
    phase: Phase,
    token: AnimationToken,
    opacity: Option<Motion>,
    completion_sent: bool,
}

/// Deterministic animation sampler. The caller supplies monotonic timestamps and
/// owns scheduling, drawing, and lifecycle. Tokens are runner-local; callers
/// must additionally check their surface generation before applying a callback.
pub struct AnimationRunner {
    plan: AnimationPlan,
    active: Option<Active>,
    last_now: Option<Duration>,
    next_token: u64,
}

impl AnimationRunner {
    pub fn new(plan: AnimationPlan) -> Result<Self, RunnerError> {
        validate_plan(plan)?;
        Ok(Self {
            plan,
            active: None,
            last_now: None,
            next_token: 1,
        })
    }

    pub fn request(&mut self, phase: Phase, now: Duration) -> Result<AnimationToken, RunnerError> {
        self.check_time(now)?;
        if let Some(active) = self.active.filter(|active| active.phase == phase) {
            self.last_now = Some(now);
            return Ok(active.token);
        }
        let token = AnimationToken(self.next_token);
        let next_token = self
            .next_token
            .checked_add(1)
            .ok_or(RunnerError::TokenExhausted)?;
        let previous = self.active;
        let opacity = match phase {
            Phase::Enter => self.plan.opacity.and_then(|c| c.enter),
            Phase::Exit => self.plan.opacity.and_then(|c| c.exit),
        };
        let opacity_span = self.plan.opacity.map(channel_span).unwrap_or(1.0);
        let make_motion = |assignment: PropertyAnimation,
                           old: Option<Motion>,
                           original_span: f64|
         -> Result<Motion, RunnerError> {
            let endpoints = assignment.endpoints;
            let (origin, span) = match old {
                Some(old) => {
                    let (position, velocity, _) = old.evaluate(now);
                    ((position, velocity), old.span)
                }
                None => ((endpoints[0] as f64, 0.0), original_span),
            };
            let target = endpoints[1] as f64;
            let finish = finish_elapsed(origin, target, span, assignment.timing);
            Ok(Motion {
                origin,
                target,
                span,
                start: now,
                timing: assignment.timing,
                finish,
            })
        };
        let active = Active {
            phase,
            token,
            opacity: opacity
                .map(|assignment| {
                    make_motion(assignment, previous.and_then(|a| a.opacity), opacity_span)
                })
                .transpose()?,
            completion_sent: false,
        };
        self.next_token = next_token;
        self.last_now = Some(now);
        self.active = Some(active);
        Ok(token)
    }

    pub fn sample(&mut self, now: Duration) -> Result<AnimationSample, RunnerError> {
        self.check_time(now)?;
        let Some(mut active) = self.active else {
            self.last_now = Some(now);
            return Ok(AnimationSample {
                opacity: None,
                running: false,
                completion: None,
            });
        };
        let (opacity, running) = sample_motion(active.opacity.as_ref(), now);
        let completion = if !running && !active.completion_sent {
            active.completion_sent = true;
            Some(active.token)
        } else {
            None
        };
        self.active = Some(active);
        self.last_now = Some(now);
        Ok(AnimationSample {
            opacity,
            running,
            completion,
        })
    }

    pub fn cancel(&mut self) {
        self.active = None;
    }

    pub fn is_current(&self, token: AnimationToken) -> bool {
        self.active.is_some_and(|active| active.token == token)
    }

    fn check_time(&self, now: Duration) -> Result<(), RunnerError> {
        if self.last_now.is_some_and(|last| now < last) {
            Err(RunnerError::NonMonotonicTime)
        } else {
            Ok(())
        }
    }
}

fn sample_motion(motion: Option<&Motion>, now: Duration) -> (Option<f32>, bool) {
    let Some(motion) = motion else {
        return (None, false);
    };
    let (value, _, running) = motion.evaluate(now);
    (Some(value.clamp(0.0, 1.0) as f32), running)
}

fn channel_span(channel: Channel) -> f64 {
    let span = |assignment: Option<PropertyAnimation>| {
        assignment.map_or(0.0, |assignment| {
            (assignment.endpoints[1] as f64 - assignment.endpoints[0] as f64).abs()
        })
    };
    span(channel.enter).max(span(channel.exit)).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn easing(ms: u64) -> Option<PlanTiming> {
        Some(PlanTiming::Easing {
            duration: Duration::from_millis(ms),
            curve: EasingCurve::Linear,
        })
    }

    fn spring(k: f32, c: f32, m: f32, max_ms: u64) -> Option<PlanTiming> {
        Some(PlanTiming::Spring {
            stiffness: k,
            damping: c,
            mass: m,
            max_duration: Duration::from_millis(max_ms),
        })
    }

    fn assignment(from: f32, to: f32, timing: Option<PlanTiming>) -> Option<PropertyAnimation> {
        Some(PropertyAnimation {
            endpoints: [from, to],
            timing,
        })
    }

    fn channel(
        enter: Option<PropertyAnimation>,
        exit: Option<PropertyAnimation>,
    ) -> Option<Channel> {
        Some(Channel { enter, exit })
    }

    fn plan(opacity: Option<Channel>) -> AnimationPlan {
        AnimationPlan { opacity }
    }

    #[test]
    fn absent_phase_keeps_its_immediate_assignment_separate_from_other_timing() {
        let plan = plan(channel(
            assignment(0.0, 1.0, easing(100)),
            assignment(1.0, 1.0, None),
        ));
        let mut runner = AnimationRunner::new(plan).unwrap();
        runner.request(Phase::Enter, Duration::ZERO).unwrap();
        runner.sample(Duration::from_millis(50)).unwrap();
        runner
            .request(Phase::Exit, Duration::from_millis(50))
            .unwrap();
        assert_eq!(runner.active.unwrap().opacity.unwrap().timing, None);
    }

    #[test]
    fn easing_velocity_is_preserved_when_retargeted_to_spring() {
        let plan = plan(channel(
            assignment(0.0, 1.0, easing(100)),
            assignment(1.0, 0.0, spring(100.0, 10.0, 1.0, 2000)),
        ));
        let mut runner = AnimationRunner::new(plan).unwrap();
        runner.request(Phase::Enter, Duration::ZERO).unwrap();
        let before = runner.sample(Duration::from_millis(50)).unwrap();
        let state = runner
            .active
            .unwrap()
            .opacity
            .unwrap()
            .evaluate(Duration::from_millis(50));
        assert_eq!(state, (0.5, 10.0, true));
        runner
            .request(Phase::Exit, Duration::from_millis(50))
            .unwrap();
        assert_eq!(
            runner.active.unwrap().opacity.unwrap().origin,
            (state.0, state.1)
        );
        assert_eq!(
            runner.sample(Duration::from_millis(50)).unwrap().opacity,
            before.opacity
        );
    }

    #[test]
    fn fps_history_has_identical_common_time_position_velocity_and_completion_status() {
        let timing = spring(300.0, 30.0, 1.0, 2000);
        let plan = plan(channel(
            assignment(0.0, 1.0, timing),
            assignment(1.0, 0.0, timing),
        ));
        let common = [50, 100, 250, 500, 750, 1000, 1500, 2000];
        let mut trajectories = Vec::new();
        for fps in [30_u32, 60, 144] {
            let mut runner = AnimationRunner::new(plan).unwrap();
            runner.request(Phase::Enter, Duration::ZERO).unwrap();
            let mut frame = 1_u32;
            let mut last = Duration::ZERO;
            let mut completion_seen = false;
            let mut points = Vec::new();
            for millis in common {
                let time = Duration::from_millis(millis);
                while Duration::from_secs_f64(f64::from(frame) / f64::from(fps)) < time {
                    let tick = Duration::from_secs_f64(f64::from(frame) / f64::from(fps));
                    if tick > last {
                        completion_seen |= runner.sample(tick).unwrap().completion.is_some();
                        last = tick;
                    }
                    frame += 1;
                }
                let sample = runner.sample(time).unwrap();
                completion_seen |= sample.completion.is_some();
                last = time;
                let state = runner.active.unwrap().opacity.unwrap().evaluate(time);
                points.push((state.0, state.1, sample.running, completion_seen));
            }
            trajectories.push(points);
        }
        for index in 0..common.len() {
            let expected = trajectories[0][index];
            for points in &trajectories[1..] {
                let actual = points[index];
                assert!(
                    (actual.0 - expected.0).abs() < 1e-12,
                    "{}ms {actual:?} {expected:?}",
                    common[index]
                );
                assert!(
                    (actual.1 - expected.1).abs() < 1e-10,
                    "{}ms {actual:?} {expected:?}",
                    common[index]
                );
                assert_eq!((actual.2, actual.3), (expected.2, expected.3));
            }
        }
        assert!(trajectories[0].iter().any(|point| point.3));
    }

    #[test]
    fn channel_span_is_original_across_phases_and_retargets() {
        for (enter, exit, span) in [([0.0, 0.5], [0.5, 0.0], 1.0), ([0.0, 1.0], [1.0, 0.0], 1.0)] {
            let timing = spring(10.0, 2.0, 1.0, 2000);
            let property = |pair: [f32; 2]| assignment(pair[0], pair[1], timing);
            let c = channel(property(enter), property(exit));
            let plan = plan(c);
            let mut runner = AnimationRunner::new(plan).unwrap();
            runner.request(Phase::Exit, Duration::ZERO).unwrap();
            let motion = |runner: &AnimationRunner| runner.active.unwrap().opacity.unwrap();
            assert_eq!(motion(&runner).span, span);
            runner.sample(Duration::from_millis(50)).unwrap();
            runner
                .request(Phase::Enter, Duration::from_millis(50))
                .unwrap();
            assert_eq!(motion(&runner).span, span);
            runner.sample(Duration::from_millis(100)).unwrap();
            runner
                .request(Phase::Exit, Duration::from_millis(100))
                .unwrap();
            assert_eq!(motion(&runner).span, span);
        }
    }

    #[test]
    fn opacity_clamp_never_mutates_the_solver_state_when_retargeted() {
        let timing = spring(100.0, 1.0, 1.0, 2000);
        let plan = plan(channel(
            assignment(0.0, 1.0, timing),
            assignment(1.0, 0.0, timing),
        ));
        let mut runner = AnimationRunner::new(plan).unwrap();
        runner.request(Phase::Enter, Duration::ZERO).unwrap();
        let sample = runner.sample(Duration::from_millis(200)).unwrap();
        assert_eq!(sample.opacity, Some(1.0));
        let opacity = runner.active.unwrap().opacity.unwrap();
        assert!(opacity.evaluate(Duration::from_millis(200)).0 > 1.0);
        runner
            .request(Phase::Exit, Duration::from_millis(200))
            .unwrap();
        assert_eq!(
            runner.active.unwrap().opacity.unwrap().origin,
            (
                opacity.evaluate(Duration::from_millis(200)).0,
                opacity.evaluate(Duration::from_millis(200)).1
            )
        );
    }
}

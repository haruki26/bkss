use std::time::Duration;

use bkss_core::{
    AnimationPlan, AnimationRunner, Channel, EasingCurve, Phase, PlanTiming, PropertyAnimation,
    RunnerError,
};

#[derive(Default)]
struct FakeClock(Duration);

impl FakeClock {
    fn set(&mut self, value: Duration) -> Duration {
        self.0 = value;
        self.0
    }
}

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

fn channel(enter: Option<PropertyAnimation>, exit: Option<PropertyAnimation>) -> Option<Channel> {
    Some(Channel { enter, exit })
}

fn plan(opacity: Option<Channel>) -> AnimationPlan {
    AnimationPlan {
        opacity,
        ..AnimationPlan::default()
    }
}

#[test]
fn validates_all_property_timings_and_endpoints() {
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0] {
        for timing in [
            spring(value, 1.0, 1.0, 1),
            spring(1.0, value, 1.0, 1),
            spring(1.0, 1.0, value, 1),
        ] {
            assert_eq!(
                AnimationRunner::new(plan(channel(assignment(0.0, 1.0, timing), None))).err(),
                Some(RunnerError::InvalidPlan)
            );
        }
    }
    assert_eq!(
        AnimationRunner::new(plan(channel(
            assignment(0.0, 1.0, spring(1.0, 1.0, 1.0, 0)),
            None
        ),))
        .err(),
        Some(RunnerError::InvalidPlan)
    );
    for endpoints in [
        [f32::NAN, 1.0],
        [f32::INFINITY, 1.0],
        [f32::NEG_INFINITY, 1.0],
        [-0.1, 1.0],
        [0.0, 1.1],
    ] {
        assert_eq!(
            AnimationRunner::new(plan(channel(
                assignment(endpoints[0], endpoints[1], easing(10)),
                None
            ),))
            .err(),
            Some(RunnerError::InvalidPlan)
        );
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for (translation_x, translation_y) in [
            (channel(assignment(0.0, value, easing(10)), None), None),
            (None, channel(assignment(0.0, value, easing(10)), None)),
        ] {
            assert_eq!(
                AnimationRunner::new(AnimationPlan {
                    translation_x,
                    translation_y,
                    ..AnimationPlan::default()
                })
                .err(),
                Some(RunnerError::InvalidPlan)
            );
        }
    }
    assert!(
        AnimationRunner::new(AnimationPlan {
            translation_x: channel(assignment(-200.0, 350.0, easing(10)), None),
            ..AnimationPlan::default()
        })
        .is_ok()
    );
    assert!(
        AnimationRunner::new(plan(channel(
            assignment(0.0, 1.0, spring(1e20, 1e20, 1e20, 10)),
            None
        ),))
        .is_ok()
    );
    assert!(
        AnimationRunner::new(plan(channel(
            assignment(
                0.0,
                1.0,
                spring(f32::from_bits(1), f32::from_bits(1), f32::from_bits(1), 10)
            ),
            None
        ),))
        .is_ok()
    );
}

#[test]
fn absent_phase_is_immediate() {
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
    let sample = runner.sample(Duration::from_millis(50)).unwrap();
    assert_eq!(sample.opacity, Some(1.0));
    assert!(!sample.running);
}

#[test]
fn huge_phase_and_subnormal_physics_are_accepted() {
    let timing = spring(f32::MAX, f32::MIN_POSITIVE, f32::MIN_POSITIVE, 100);
    let mut runner = AnimationRunner::new(plan(channel(
        assignment(0.0, 1.0, timing),
        assignment(1.0, 0.0, timing),
    )))
    .unwrap();
    runner.request(Phase::Enter, Duration::ZERO).unwrap();
    assert!(
        runner
            .sample(Duration::from_nanos(1))
            .unwrap()
            .opacity
            .unwrap()
            .is_finite()
    );
    let subnormal = spring(f32::from_bits(1), f32::from_bits(1), f32::MAX, 100);
    assert!(
        AnimationRunner::new(plan(channel(
            assignment(0.0, 1.0, subnormal),
            assignment(1.0, 0.0, subnormal)
        ),))
        .is_ok()
    );
}

#[test]
fn request_watermark_deadline_completion_and_token_lifetime() {
    let timing = spring(0.0001, 0.0001, 1.0, 1000);
    let mut clock = FakeClock::default();
    let mut runner = AnimationRunner::new(plan(channel(
        assignment(0.0, 1.0, timing),
        assignment(1.0, 0.0, timing),
    )))
    .unwrap();
    let token = runner
        .request(Phase::Enter, clock.set(Duration::ZERO))
        .unwrap();
    assert_eq!(
        runner
            .request(Phase::Enter, clock.set(Duration::from_millis(800)))
            .unwrap(),
        token
    );
    assert_eq!(
        runner.sample(Duration::from_millis(799)),
        Err(RunnerError::NonMonotonicTime)
    );
    assert!(
        runner
            .sample(clock.set(Duration::from_millis(999)))
            .unwrap()
            .running
    );
    let done = runner.sample(clock.set(Duration::from_secs(1))).unwrap();
    assert_eq!(done.completion, Some(token));
    assert!(runner.is_current(token));
    assert_eq!(
        runner
            .sample(clock.set(Duration::from_millis(1500)))
            .unwrap()
            .completion,
        None
    );
    let next = runner
        .request(Phase::Exit, clock.set(Duration::from_millis(1500)))
        .unwrap();
    assert!(!runner.is_current(token));
    runner.cancel();
    assert!(!runner.is_current(next));
}

#[test]
fn fps_history_has_identical_common_time_position_and_completion_status() {
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
            points.push((sample.opacity, sample.running, completion_seen));
        }
        trajectories.push(points);
    }
    for index in 0..common.len() {
        let expected = trajectories[0][index];
        for points in &trajectories[1..] {
            assert_eq!(points[index], expected, "{}ms", common[index]);
        }
    }
    assert!(trajectories[0].iter().any(|point| point.2));
}

#[test]
fn absent_channels_are_immediate_and_completion_is_once() {
    let mut runner = AnimationRunner::new(plan(None)).unwrap();
    let token = runner.request(Phase::Enter, Duration::ZERO).unwrap();
    let sample = runner.sample(Duration::ZERO).unwrap();
    assert!(!sample.running);
    assert_eq!(sample.completion, Some(token));
    assert!(runner.is_current(token));
    assert_eq!(
        runner.sample(Duration::from_secs(1)).unwrap().completion,
        None
    );
}

#[test]
fn translations_share_phase_token_and_complete_after_all_channels() {
    let x = channel(
        assignment(0.0, 100.0, easing(100)),
        assignment(100.0, 0.0, easing(100)),
    );
    let y = channel(
        assignment(0.0, 50.0, easing(200)),
        assignment(50.0, 0.0, easing(200)),
    );
    let mut runner = AnimationRunner::new(AnimationPlan {
        translation_x: x,
        translation_y: y,
        ..AnimationPlan::default()
    })
    .unwrap();
    let token = runner.request(Phase::Enter, Duration::ZERO).unwrap();
    let halfway = runner.sample(Duration::from_millis(100)).unwrap();
    assert_eq!(halfway.translation_x, Some(100.0));
    assert_eq!(halfway.translation_y, Some(25.0));
    assert!(halfway.running);
    assert_eq!(halfway.completion, None);
    let done = runner.sample(Duration::from_millis(200)).unwrap();
    assert_eq!(done.translation_y, Some(50.0));
    assert!(!done.running);
    assert_eq!(done.completion, Some(token));
    assert_eq!(
        runner
            .sample(Duration::from_millis(300))
            .unwrap()
            .completion,
        None
    );
}

#[test]
fn spring_translation_preserves_overshoot_and_retarget_state() {
    let timing = spring(100.0, 1.0, 1.0, 2000);
    let mut runner = AnimationRunner::new(AnimationPlan {
        translation_x: channel(assignment(0.0, 1.0, timing), assignment(1.0, 0.0, timing)),
        ..AnimationPlan::default()
    })
    .unwrap();
    runner.request(Phase::Enter, Duration::ZERO).unwrap();
    let time = Duration::from_millis(200);
    let sample = runner.sample(time).unwrap();
    assert!(
        sample.translation_x.unwrap() > 1.0,
        "expected spring overshoot"
    );

    runner.request(Phase::Exit, time).unwrap();
    assert_eq!(
        runner.sample(time).unwrap().translation_x,
        sample.translation_x
    );
}

#[test]
fn translation_samples_reject_f32_overflow_in_both_directions() {
    let timing = spring(100.0, 1.0, 1.0, 2000);
    for endpoint in [f32::MAX, -f32::MAX] {
        let mut runner = AnimationRunner::new(AnimationPlan {
            translation_x: channel(
                assignment(0.0, endpoint, timing),
                assignment(endpoint, 0.0, timing),
            ),
            ..AnimationPlan::default()
        })
        .unwrap();
        runner.request(Phase::Enter, Duration::ZERO).unwrap();
        assert_eq!(
            runner.sample(Duration::from_millis(200)),
            Err(RunnerError::InvalidPlan)
        );
    }
}

#[test]
fn submillisecond_easing_duration_remains_non_immediate() {
    let mut runner = AnimationRunner::new(plan(channel(
        assignment(
            0.0,
            1.0,
            Some(PlanTiming::Easing {
                duration: Duration::from_micros(500),
                curve: EasingCurve::Linear,
            }),
        ),
        None,
    )))
    .unwrap();
    runner.request(Phase::Enter, Duration::ZERO).unwrap();
    let halfway = runner.sample(Duration::from_micros(250)).unwrap();
    assert_eq!(halfway.opacity, Some(0.5));
    assert!(halfway.running);
    assert_eq!(
        runner.sample(Duration::from_micros(500)).unwrap().opacity,
        Some(1.0)
    );
}

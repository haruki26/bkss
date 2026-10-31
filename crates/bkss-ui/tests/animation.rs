use std::time::Duration;

use bkss_core::{EasingCurve as CoreEasingCurve, PlanTiming};
use bkss_ui::{Animation, Timing};

fn default_plan_timing() -> Option<PlanTiming> {
    Some(PlanTiming::Easing {
        duration: Duration::from_millis(160),
        curve: CoreEasingCurve::EaseOut,
    })
}

fn spring_default_plan_timing() -> Option<PlanTiming> {
    Some(PlanTiming::Spring {
        stiffness: 300.0,
        damping: 30.0,
        mass: 1.0,
        max_duration: Duration::from_millis(2000),
    })
}

#[test]
fn transition_compiles_reverse_channels_and_default_timing() {
    let plan = Animation::transition(Timing::default())
        .opacity(0.0, 1.0)
        .compile()
        .unwrap();
    let opacity = plan.opacity.unwrap();
    assert_eq!(opacity.enter.unwrap().endpoints, [0.0, 1.0]);
    assert_eq!(opacity.exit.unwrap().endpoints, [1.0, 0.0]);
    assert_eq!(
        opacity.enter.unwrap().timing,
        Some(PlanTiming::Easing {
            duration: Duration::from_millis(160),
            curve: CoreEasingCurve::EaseOut,
        })
    );
}

#[test]
fn later_property_assignment_wins_during_merge() {
    let plan = Animation::transition(Timing::default())
        .opacity(0.0, 1.0)
        .merge(Animation::transition(Timing::default()).opacity(0.2, 0.9))
        .compile()
        .unwrap();
    assert_eq!(plan.opacity.unwrap().enter.unwrap().endpoints, [0.2, 0.9]);
}

#[test]
fn merge_keeps_both_phases_and_later_phase_timing() {
    let plan = Animation::enter(Timing::default())
        .opacity(0.0, 1.0)
        .merge(Animation::exit(Timing::spring_default()).opacity(1.0, 0.25))
        .compile()
        .unwrap();
    let channel = plan.opacity.unwrap();
    assert_eq!(channel.enter.unwrap().endpoints, [0.0, 1.0]);
    assert_eq!(channel.exit.unwrap().endpoints, [1.0, 0.25]);
    assert_eq!(channel.enter.unwrap().timing, default_plan_timing());
    assert_eq!(channel.exit.unwrap().timing, spring_default_plan_timing());
}

#[test]
fn later_exit_definition_clears_transition_reversal_for_later_property_assignment() {
    let plan = Animation::transition(Timing::default())
        .opacity(0.0, 1.0)
        .merge(Animation::exit(Timing::default()))
        .opacity(0.2, 0.8)
        .compile()
        .unwrap();
    assert_eq!(plan.opacity.unwrap().exit.unwrap().endpoints, [0.2, 0.8]);
}

#[test]
fn phase_timing_is_captured_per_assignment_and_merge_preserves_untouched_phase() {
    let plan = Animation::enter(Timing::default())
        .opacity(0.0, 1.0)
        .merge(Animation::exit(Timing::spring_default()).opacity(1.0, 0.1))
        .compile()
        .unwrap();
    let opacity = plan.opacity.unwrap();
    assert_eq!(opacity.enter.unwrap().timing, default_plan_timing());
    assert_eq!(opacity.exit.unwrap().timing, spring_default_plan_timing());
}

use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Enter,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlanTiming {
    Easing {
        duration: Duration,
        curve: EasingCurve,
    },
    Spring {
        stiffness: f32,
        damping: f32,
        mass: f32,
        max_duration: Duration,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasingCurve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropertyAnimation {
    pub endpoints: [f32; 2],
    pub timing: Option<PlanTiming>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Channel {
    pub enter: Option<PropertyAnimation>,
    pub exit: Option<PropertyAnimation>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationPlan {
    pub opacity: Option<Channel>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnimationToken(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationSample {
    pub opacity: Option<f32>,
    pub running: bool,
    pub completion: Option<AnimationToken>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerError {
    InvalidPlan,
    NonMonotonicTime,
    TokenExhausted,
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidPlan => f.write_str("invalid animation plan or solver state"),
            Self::NonMonotonicTime => f.write_str("animation timestamp is not monotonic"),
            Self::TokenExhausted => f.write_str("animation token space exhausted"),
        }
    }
}

impl std::error::Error for RunnerError {}

pub(super) fn validate_plan(plan: AnimationPlan) -> Result<(), RunnerError> {
    let valid_timing = |timing: Option<PlanTiming>| {
        timing.is_none_or(|timing| match timing {
            PlanTiming::Easing { .. } => true,
            PlanTiming::Spring {
                stiffness,
                damping,
                mass,
                max_duration,
            } => {
                stiffness.is_finite()
                    && stiffness > 0.0
                    && damping.is_finite()
                    && damping > 0.0
                    && mass.is_finite()
                    && mass > 0.0
                    && !max_duration.is_zero()
            }
        })
    };
    let valid_assignment = |assignment: Option<PropertyAnimation>| {
        assignment.is_none_or(|assignment| {
            let [from, to] = assignment.endpoints;
            let endpoints_valid = from.is_finite()
                && to.is_finite()
                && (0.0..=1.0).contains(&from)
                && (0.0..=1.0).contains(&to);
            endpoints_valid && valid_timing(assignment.timing)
        })
    };
    let valid_channel = |channel: Option<Channel>| {
        channel
            .is_none_or(|channel| valid_assignment(channel.enter) && valid_assignment(channel.exit))
    };
    if valid_channel(plan.opacity) {
        Ok(())
    } else {
        Err(RunnerError::InvalidPlan)
    }
}

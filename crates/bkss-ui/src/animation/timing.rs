use std::time::Duration;

use bkss_core::{EasingCurve as CoreEasingCurve, PlanTiming};

use super::error::AnimationError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EasingCurve {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timing {
    Easing {
        duration_ms: u64,
        curve: EasingCurve,
    },
    Spring {
        stiffness: f32,
        damping: f32,
        mass: f32,
        max_duration_ms: u64,
    },
}

impl Default for Timing {
    fn default() -> Self {
        Self::Easing {
            duration_ms: 160,
            curve: EasingCurve::EaseOut,
        }
    }
}

impl Timing {
    pub const fn spring_default() -> Self {
        Self::Spring {
            stiffness: 300.0,
            damping: 30.0,
            mass: 1.0,
            max_duration_ms: 2000,
        }
    }

    pub(super) fn compile(self) -> Result<PlanTiming, AnimationError> {
        match self {
            Self::Easing { duration_ms, curve } => Ok(PlanTiming::Easing {
                duration: Duration::from_millis(duration_ms),
                curve: match curve {
                    EasingCurve::Linear => CoreEasingCurve::Linear,
                    EasingCurve::EaseIn => CoreEasingCurve::EaseIn,
                    EasingCurve::EaseOut => CoreEasingCurve::EaseOut,
                    EasingCurve::EaseInOut => CoreEasingCurve::EaseInOut,
                },
            }),
            Self::Spring {
                stiffness,
                damping,
                mass,
                max_duration_ms,
            } => {
                if !stiffness.is_finite()
                    || stiffness <= 0.0
                    || !damping.is_finite()
                    || damping <= 0.0
                    || !mass.is_finite()
                    || mass <= 0.0
                    || max_duration_ms == 0
                {
                    return Err(AnimationError::InvalidTiming);
                }
                Ok(PlanTiming::Spring {
                    stiffness,
                    damping,
                    mass,
                    max_duration: Duration::from_millis(max_duration_ms),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnimationError, Timing};

    #[test]
    fn rejects_invalid_spring_parameters_and_accepts_finite_extremes() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0] {
            for timing in [
                Timing::Spring {
                    stiffness: value,
                    damping: 1.0,
                    mass: 1.0,
                    max_duration_ms: 1,
                },
                Timing::Spring {
                    stiffness: 1.0,
                    damping: value,
                    mass: 1.0,
                    max_duration_ms: 1,
                },
                Timing::Spring {
                    stiffness: 1.0,
                    damping: 1.0,
                    mass: value,
                    max_duration_ms: 1,
                },
            ] {
                assert_eq!(timing.compile().err(), Some(AnimationError::InvalidTiming));
            }
        }
        assert_eq!(
            Timing::Spring {
                stiffness: 1.0,
                damping: 1.0,
                mass: 1.0,
                max_duration_ms: 0,
            }
            .compile()
            .err(),
            Some(AnimationError::InvalidTiming)
        );
        assert!(
            Timing::Spring {
                stiffness: 1.0,
                damping: f32::MAX,
                mass: 1.0,
                max_duration_ms: 10,
            }
            .compile()
            .is_ok()
        );
    }
}

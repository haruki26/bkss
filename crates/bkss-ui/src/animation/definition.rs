use bkss_core::{AnimationPlan, Channel, PropertyAnimation};

use super::error::AnimationError;
use super::timing::Timing;

/// Inert opacity definition. It never starts a clock, task, or animation.
#[derive(Clone, Copy, Debug)]
pub struct Animation {
    enter_timing: Option<Timing>,
    exit_timing: Option<Timing>,
    reverse_exit: bool,
    enter_opacity: Option<Assignment>,
    exit_opacity: Option<Assignment>,
}

#[derive(Clone, Copy, Debug)]
struct Assignment {
    endpoints: [f32; 2],
    timing: Option<Timing>,
}

impl Animation {
    pub const fn enter(timing: Timing) -> Self {
        Self {
            enter_timing: Some(timing),
            exit_timing: None,
            reverse_exit: false,
            enter_opacity: None,
            exit_opacity: None,
        }
    }
    pub const fn exit(timing: Timing) -> Self {
        Self {
            enter_timing: None,
            exit_timing: Some(timing),
            reverse_exit: false,
            enter_opacity: None,
            exit_opacity: None,
        }
    }
    pub const fn transition(timing: Timing) -> Self {
        Self {
            enter_timing: Some(timing),
            exit_timing: Some(timing),
            reverse_exit: true,
            enter_opacity: None,
            exit_opacity: None,
        }
    }

    pub fn opacity(mut self, from: f32, to: f32) -> Self {
        if let Some(timing) = self.enter_timing {
            self.enter_opacity = Some(Assignment {
                endpoints: [from, to],
                timing: Some(timing),
            });
        }
        if let Some(timing) = self.exit_timing {
            self.exit_opacity = Some(Assignment {
                endpoints: if self.reverse_exit {
                    [to, from]
                } else {
                    [from, to]
                },
                timing: Some(timing),
            });
        }
        self
    }
    /// Applies the later definition's direction, timing, and assigned properties.
    pub fn merge(self, later: Self) -> Self {
        Self {
            enter_timing: later.enter_timing.or(self.enter_timing),
            exit_timing: later.exit_timing.or(self.exit_timing),
            reverse_exit: if later.exit_timing.is_some() {
                later.reverse_exit
            } else {
                self.reverse_exit
            },
            enter_opacity: later.enter_opacity.or(self.enter_opacity),
            exit_opacity: later.exit_opacity.or(self.exit_opacity),
        }
    }

    /// Validates the complete definition and compiles the narrowly scoped core plan.
    #[doc(hidden)]
    pub fn compile(self) -> Result<AnimationPlan, AnimationError> {
        self.enter_timing.map(Timing::compile).transpose()?;
        self.exit_timing.map(Timing::compile).transpose()?;
        let channel = |enter: Option<Assignment>,
                       exit: Option<Assignment>|
         -> Result<Option<Channel>, AnimationError> {
            let (Some(enter), Some(exit)) = (enter, exit) else {
                return Ok(None);
            };
            let compile = |assignment: Assignment| -> Result<PropertyAnimation, AnimationError> {
                let pair = assignment.endpoints;
                if pair
                    .into_iter()
                    .any(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
                {
                    return Err(AnimationError::InvalidEndpoint);
                }
                Ok(PropertyAnimation {
                    endpoints: pair,
                    timing: assignment.timing.map(Timing::compile).transpose()?,
                })
            };
            Ok(Some(Channel {
                enter: Some(compile(enter)?),
                exit: Some(compile(exit)?),
            }))
        };
        let fill_missing = |enter: Option<Assignment>, exit: Option<Assignment>| match (enter, exit)
        {
            (Some(enter), Some(exit)) => (Some(enter), Some(exit)),
            (Some(enter), None) => (
                Some(enter),
                Some(Assignment {
                    endpoints: [enter.endpoints[1]; 2],
                    timing: None,
                }),
            ),
            (None, Some(exit)) => (
                Some(Assignment {
                    endpoints: [exit.endpoints[0]; 2],
                    timing: None,
                }),
                Some(exit),
            ),
            (None, None) => (None, None),
        };
        let (enter_opacity, exit_opacity) = fill_missing(self.enter_opacity, self.exit_opacity);
        Ok(AnimationPlan {
            opacity: channel(enter_opacity, exit_opacity)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_definitions_fallback_phases_and_endpoint_validation_are_preserved() {
        assert!(
            Animation::enter(Timing::spring_default())
                .compile()
                .unwrap()
                .opacity
                .is_none()
        );
        assert_eq!(
            Animation::enter(Timing::Spring {
                stiffness: 0.0,
                damping: 1.0,
                mass: 1.0,
                max_duration_ms: 10,
            })
            .compile()
            .err(),
            Some(AnimationError::InvalidTiming)
        );
        assert_eq!(
            Animation::exit(Timing::Spring {
                stiffness: 1.0,
                damping: 1.0,
                mass: 1.0,
                max_duration_ms: 0,
            })
            .compile()
            .err(),
            Some(AnimationError::InvalidTiming)
        );
        assert!(
            Animation::enter(Timing::Spring {
                stiffness: 1.0,
                damping: f32::MAX,
                mass: 1.0,
                max_duration_ms: 10,
            })
            .compile()
            .is_ok()
        );
        let enter_only = Animation::enter(Timing::default())
            .opacity(0.0, 1.0)
            .compile()
            .unwrap();
        assert_eq!(
            enter_only.opacity.unwrap().exit.unwrap().endpoints,
            [1.0, 1.0]
        );
        assert_eq!(enter_only.opacity.unwrap().exit.unwrap().timing, None);
        let exit_only = Animation::exit(Timing::default())
            .opacity(1.0, 0.0)
            .compile()
            .unwrap();
        assert_eq!(
            exit_only.opacity.unwrap().enter.unwrap().endpoints,
            [1.0, 1.0]
        );
        assert_eq!(exit_only.opacity.unwrap().enter.unwrap().timing, None);

        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            assert_eq!(
                Animation::enter(Timing::default())
                    .opacity(value, 1.0)
                    .compile()
                    .err(),
                Some(AnimationError::InvalidEndpoint),
            );
        }
        assert_eq!(
            Animation::transition(Timing::default())
                .opacity(f32::NAN, 1.0)
                .compile()
                .err(),
            Some(AnimationError::InvalidEndpoint),
        );
    }
}

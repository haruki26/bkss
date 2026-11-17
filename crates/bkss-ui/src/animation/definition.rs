use std::time::Duration;

use bkss_core::{AnimationPlan, Channel, PropertyAnimation};
use gpui::Size;

use super::error::AnimationError;

/// A preset for the core spring solver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpringPreset {
    Smooth,
    Snappy,
    Bouncy,
}

impl SpringPreset {
    fn timing(self) -> bkss_core::PlanTiming {
        let (stiffness, damping, mass) = match self {
            Self::Smooth => (180.0, 24.0, 1.0),
            Self::Snappy => (300.0, 30.0, 1.0),
            Self::Bouncy => (300.0, 18.0, 1.0),
        };
        bkss_core::PlanTiming::Spring {
            stiffness,
            damping,
            mass,
            max_duration: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Property {
    enter: Option<[f32; 2]>,
    exit: Option<[f32; 2]>,
}

#[derive(Clone, Copy, Debug)]
enum TransitionTiming {
    Duration(Duration),
    Timing(bkss_core::PlanTiming),
}

impl TransitionTiming {
    fn compile(self) -> bkss_core::PlanTiming {
        match self {
            Self::Duration(duration) => bkss_core::PlanTiming::Easing {
                duration,
                curve: bkss_core::EasingCurve::EaseOut,
            },
            Self::Timing(timing) => timing,
        }
    }
}

/// Inert declaration of a transition. Compose definitions with [`Transition::and`].
#[derive(Clone, Copy, Debug)]
pub struct Transition {
    timing: Option<TransitionTiming>,
    opacity: Option<Property>,
    translation_x: Option<Property>,
    translation_y: Option<Property>,
    distance: Option<f32>,
}

impl Transition {
    const fn empty() -> Self {
        Self {
            timing: None,
            opacity: None,
            translation_x: None,
            translation_y: None,
            distance: None,
        }
    }

    /// Fades from transparent to opaque on enter, and reverses on exit.
    pub const fn fade() -> Self {
        Self {
            opacity: Some(Property {
                enter: Some([0.0, 1.0]),
                exit: Some([1.0, 0.0]),
            }),
            ..Self::empty()
        }
    }

    /// Explicit exit-only fade from opaque to transparent.
    pub const fn fade_out() -> Self {
        Self {
            opacity: Some(Property {
                enter: None,
                exit: Some([1.0, 0.0]),
            }),
            ..Self::empty()
        }
    }

    /// Slides content upward while entering; exit moves it downward.
    pub const fn slide_up() -> Self {
        Self::slide_axis(true, false)
    }

    /// Slides content downward while entering; exit moves it upward.
    pub const fn slide_down() -> Self {
        Self::slide_axis(true, true)
    }

    /// Slides content from the right while entering; exit moves it rightward.
    pub const fn slide_left() -> Self {
        Self::slide_axis(false, false)
    }

    /// Slides content from the left while entering; exit moves it leftward.
    pub const fn slide_right() -> Self {
        Self::slide_axis(false, true)
    }

    const fn slide_axis(vertical: bool, from_negative: bool) -> Self {
        let sign = if from_negative { -1.0 } else { 1.0 };
        let property = Property {
            enter: Some([sign, 0.0]),
            exit: Some([0.0, sign]),
        };
        if vertical {
            Self {
                translation_y: Some(property),
                ..Self::empty()
            }
        } else {
            Self {
                translation_x: Some(property),
                ..Self::empty()
            }
        }
    }

    /// Sets a finite, non-negative logical-pixel slide distance.
    pub const fn distance(mut self, distance: f32) -> Self {
        self.distance = Some(distance);
        self
    }

    /// Sets a fixed duration for all properties in this definition.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.timing = Some(TransitionTiming::Duration(duration));
        self
    }

    /// Configures the core spring solver with the selected preset.
    pub fn spring(mut self, preset: SpringPreset) -> Self {
        self.timing = Some(TransitionTiming::Timing(preset.timing()));
        self
    }

    /// Combines definitions; later assignments for the same property win.
    pub fn and(mut self, later: Self) -> Self {
        if later.timing.is_some() {
            self.timing = later.timing;
        }
        if later.distance.is_some() {
            self.distance = later.distance;
        }
        if later.opacity.is_some() {
            self.opacity = later.opacity;
        }
        if later.translation_x.is_some() {
            self.translation_x = later.translation_x;
        }
        if later.translation_y.is_some() {
            self.translation_y = later.translation_y;
        }
        self
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SurfaceTransitions {
    enter: Option<Transition>,
    exit: Option<Transition>,
}

impl SurfaceTransitions {
    pub(crate) fn set_transition(&mut self, transition: Transition) -> &mut Self {
        self.enter = Some(transition);
        self.exit = Some(transition);
        self
    }

    pub(crate) fn set_enter(&mut self, transition: Transition) -> &mut Self {
        self.enter = Some(transition);
        self
    }

    pub(crate) fn set_exit(&mut self, transition: Transition) -> &mut Self {
        self.exit = Some(transition);
        self
    }

    pub(crate) fn requires_measurement(&self) -> bool {
        [self.enter, self.exit]
            .into_iter()
            .flatten()
            .any(|transition| {
                transition.distance.is_none()
                    && (transition.translation_x.is_some() || transition.translation_y.is_some())
            })
    }

    pub(crate) fn resolve(
        &self,
        content_size: Size<gpui::Pixels>,
    ) -> Result<AnimationPlan, AnimationError> {
        for transition in [self.enter, self.exit].into_iter().flatten() {
            if transition
                .distance
                .is_some_and(|distance| !distance.is_finite() || distance < 0.0)
            {
                return Err(AnimationError::InvalidDistance);
            }
        }
        let enter = self
            .enter
            .map(|transition| transition.compile(content_size, true))
            .transpose()?;
        let exit = self
            .exit
            .map(|transition| transition.compile(content_size, false))
            .transpose()?;
        Ok(AnimationPlan {
            opacity: compile_channel(
                enter.and_then(|phase| phase.opacity),
                exit.and_then(|phase| phase.opacity),
                enter.map(|phase| phase.timing),
                exit.map(|phase| phase.timing),
                1.0,
            ),
            translation_x: compile_channel(
                enter.and_then(|phase| phase.translation_x),
                exit.and_then(|phase| phase.translation_x),
                enter.map(|phase| phase.timing),
                exit.map(|phase| phase.timing),
                0.0,
            ),
            translation_y: compile_channel(
                enter.and_then(|phase| phase.translation_y),
                exit.and_then(|phase| phase.translation_y),
                enter.map(|phase| phase.timing),
                exit.map(|phase| phase.timing),
                0.0,
            ),
        })
    }
}

#[derive(Clone, Copy)]
struct PhasePlan {
    timing: Option<bkss_core::PlanTiming>,
    opacity: Option<PropertyAnimation>,
    translation_x: Option<PropertyAnimation>,
    translation_y: Option<PropertyAnimation>,
}

impl Transition {
    fn compile(
        self,
        content_size: Size<gpui::Pixels>,
        enter_phase: bool,
    ) -> Result<PhasePlan, AnimationError> {
        let timing =
            self.timing
                .map(TransitionTiming::compile)
                .unwrap_or(bkss_core::PlanTiming::Easing {
                    duration: Duration::from_millis(160),
                    curve: bkss_core::EasingCurve::EaseOut,
                });
        let property = |property: Option<Property>, axis: Option<f32>, opacity: bool| {
            let Some(property) = property else {
                return Ok(None);
            };
            let endpoints = if enter_phase {
                property.enter
            } else {
                property.exit
            };
            let endpoints = endpoints.or_else(|| {
                property
                    .enter
                    .map(|[_, end]| [end, end])
                    .or_else(|| property.exit.map(|[start, _]| [start, start]))
            });
            let Some(mut endpoints) = endpoints else {
                return Ok(None);
            };
            if let Some(axis) = axis {
                let distance = self.distance.unwrap_or(axis);
                if !distance.is_finite() || distance < 0.0 {
                    return Err(AnimationError::InvalidDistance);
                }
                endpoints = [endpoints[0] * distance, endpoints[1] * distance];
            }
            if opacity
                && endpoints
                    .into_iter()
                    .any(|value| !(0.0..=1.0).contains(&value))
            {
                return Err(AnimationError::InvalidEndpoint);
            }
            Ok(Some(PropertyAnimation {
                endpoints,
                timing: Some(timing),
            }))
        };
        Ok(PhasePlan {
            timing: Some(timing),
            opacity: property(self.opacity, None, true)?,
            translation_x: property(
                self.translation_x,
                Some(f32::from(content_size.width)),
                false,
            )?,
            translation_y: property(
                self.translation_y,
                Some(f32::from(content_size.height)),
                false,
            )?,
        })
    }
}

fn compile_channel(
    enter: Option<PropertyAnimation>,
    exit: Option<PropertyAnimation>,
    enter_timing: Option<Option<bkss_core::PlanTiming>>,
    exit_timing: Option<Option<bkss_core::PlanTiming>>,
    neutral: f32,
) -> Option<Channel> {
    if enter.is_none() && exit.is_none() {
        return None;
    }
    let neutral_for = |timing: Option<Option<bkss_core::PlanTiming>>| PropertyAnimation {
        endpoints: [neutral; 2],
        timing: timing.flatten(),
    };
    Some(Channel {
        enter: Some(enter.unwrap_or_else(|| neutral_for(enter_timing))),
        exit: Some(exit.unwrap_or_else(|| neutral_for(exit_timing))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn surface_transition_replaces_phases_and_resolves_slide_distance_and_composition() {
        let mut transitions = SurfaceTransitions::default();
        transitions.set_transition(Transition::fade().and(Transition::slide_up()));
        assert!(transitions.requires_measurement());
        let plan = transitions.resolve(Size::new(px(90.0), px(40.0))).unwrap();
        assert_eq!(plan.opacity.unwrap().enter.unwrap().endpoints, [0.0, 1.0]);
        assert_eq!(plan.opacity.unwrap().exit.unwrap().endpoints, [1.0, 0.0]);
        assert_eq!(
            plan.translation_y.unwrap().enter.unwrap().endpoints,
            [40.0, 0.0]
        );
        assert_eq!(
            plan.translation_y.unwrap().exit.unwrap().endpoints,
            [0.0, 40.0]
        );

        transitions.set_enter(Transition::slide_left().distance(18.0));
        assert_eq!(
            transitions
                .resolve(Size::new(px(90.0), px(40.0)))
                .unwrap()
                .translation_x
                .unwrap()
                .enter
                .unwrap()
                .endpoints,
            [18.0, 0.0]
        );
    }

    #[test]
    fn phase_override_order_neutral_channels_and_spring_presets_are_preserved() {
        let mut transitions = SurfaceTransitions::default();
        transitions
            .set_transition(Transition::slide_up().duration(Duration::from_millis(80)))
            .set_exit(Transition::fade_out().spring(SpringPreset::Bouncy));
        let plan = transitions.resolve(Size::new(px(90.0), px(40.0))).unwrap();
        let opacity = plan.opacity.unwrap();
        assert_eq!(opacity.enter.unwrap().endpoints, [1.0, 1.0]);
        assert_eq!(
            opacity.enter.unwrap().timing.unwrap(),
            bkss_core::PlanTiming::Easing {
                duration: Duration::from_millis(80),
                curve: bkss_core::EasingCurve::EaseOut,
            }
        );
        assert_eq!(opacity.exit.unwrap().endpoints, [1.0, 0.0]);
        assert_eq!(
            opacity.exit.unwrap().timing,
            Some(SpringPreset::Bouncy.timing())
        );
        let translation = plan.translation_y.unwrap();
        assert_eq!(translation.exit.unwrap().endpoints, [0.0, 0.0]);
        assert_eq!(
            translation.exit.unwrap().timing,
            Some(SpringPreset::Bouncy.timing())
        );

        transitions.set_enter(Transition::fade());
        assert_eq!(
            transitions
                .resolve(Size::new(px(90.0), px(40.0)))
                .unwrap()
                .translation_y,
            None
        );
    }

    #[test]
    fn transition_duration_preserves_submillisecond_precision() {
        for duration in [Duration::from_micros(500), Duration::from_micros(1_500)] {
            let mut transitions = SurfaceTransitions::default();
            transitions.set_transition(Transition::fade().duration(duration));
            let plan = transitions.resolve(Size::new(px(90.0), px(40.0))).unwrap();
            assert_eq!(
                plan.opacity.unwrap().enter.unwrap().timing,
                Some(bkss_core::PlanTiming::Easing {
                    duration,
                    curve: bkss_core::EasingCurve::EaseOut,
                })
            );
        }
    }

    #[test]
    fn missing_exit_is_immediate_and_invalid_slide_distance_is_rejected() {
        let mut transitions = SurfaceTransitions::default();
        transitions.set_enter(Transition::slide_down());
        let channel = transitions
            .resolve(Size::new(px(90.0), px(40.0)))
            .unwrap()
            .translation_y
            .unwrap();
        assert_eq!(channel.enter.unwrap().endpoints, [-40.0, 0.0]);
        assert_eq!(channel.exit.unwrap().timing, None);
        assert_eq!(
            transitions
                .set_exit(Transition::slide_left().distance(f32::NAN))
                .resolve(Size::new(px(90.0), px(40.0)))
                .err(),
            Some(AnimationError::InvalidDistance)
        );
    }
}

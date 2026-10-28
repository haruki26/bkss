use crate::{Reactive, ReactiveSubscription};
use gpui::{AppContext, Context};
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlassStyleError {
    NonFinite(&'static str),
    Negative(&'static str),
    OutOfRange(&'static str),
}

impl fmt::Display for GlassStyleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite(name) => write!(f, "{name} must be finite"),
            Self::Negative(name) => write!(f, "{name} must be nonnegative"),
            Self::OutOfRange(name) => write!(f, "{name} must be between 0 and 1"),
        }
    }
}

impl Error for GlassStyleError {}

fn nonnegative(value: f32, name: &'static str) -> Result<f32, GlassStyleError> {
    if !value.is_finite() {
        Err(GlassStyleError::NonFinite(name))
    } else if value < 0.0 {
        Err(GlassStyleError::Negative(name))
    } else {
        Ok(value)
    }
}

fn unit(value: f32, name: &'static str) -> Result<f32, GlassStyleError> {
    if !value.is_finite() {
        Err(GlassStyleError::NonFinite(name))
    } else if !(0.0..=1.0).contains(&value) {
        Err(GlassStyleError::OutOfRange(name))
    } else {
        Ok(value)
    }
}

/// A validated corner radius in logical pixels. Zero is allowed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassRadius(f32);

impl GlassRadius {
    /// Creates a finite, nonnegative radius.
    pub fn new(value: f32) -> Result<Self, GlassStyleError> {
        nonnegative(value, "radius").map(Self)
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

/// A validated alpha multiplier in the inclusive range `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassOpacity(f32);

impl GlassOpacity {
    /// Creates a finite opacity in the inclusive range `0..=1`.
    pub fn new(value: f32) -> Result<Self, GlassStyleError> {
        unit(value, "opacity").map(Self)
    }

    pub fn get(self) -> f32 {
        self.0
    }
}

/// A validated, normalized RGBA color with channels in `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassColor {
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
}

impl GlassColor {
    /// Creates a color whose four finite channels are each in `0..=1`.
    pub fn rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Result<Self, GlassStyleError> {
        Ok(Self {
            red: unit(red, "red")?,
            green: unit(green, "green")?,
            blue: unit(blue, "blue")?,
            alpha: unit(alpha, "alpha")?,
        })
    }

    pub fn red(self) -> f32 {
        self.red
    }
    pub fn green(self) -> f32 {
        self.green
    }
    pub fn blue(self) -> f32 {
        self.blue
    }
    pub fn alpha(self) -> f32 {
        self.alpha
    }
}

/// A validated outer border. Width is measured in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassBorder {
    width: f32,
    color: GlassColor,
}

impl GlassBorder {
    /// Creates a border with finite, nonnegative width; zero is allowed.
    pub fn new(width: f32, color: GlassColor) -> Result<Self, GlassStyleError> {
        Ok(Self {
            width: nonnegative(width, "border width")?,
            color,
        })
    }

    pub fn width(self) -> f32 {
        self.width
    }
    pub fn color(self) -> GlassColor {
        self.color
    }
}

/// A validated inset outline. Inset and width are measured in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassRim {
    inset: f32,
    width: f32,
    color: GlassColor,
}

impl GlassRim {
    /// Creates a rim with finite, nonnegative inset and width; zero is allowed.
    pub fn new(inset: f32, width: f32, color: GlassColor) -> Result<Self, GlassStyleError> {
        Ok(Self {
            inset: nonnegative(inset, "rim inset")?,
            width: nonnegative(width, "rim width")?,
            color,
        })
    }

    pub fn inset(self) -> f32 {
        self.inset
    }
    pub fn width(self) -> f32 {
        self.width
    }
    pub fn color(self) -> GlassColor {
        self.color
    }
}

/// A validated inset highlight or shadow. Offset and spread are signed logical pixels;
/// blur is finite and nonnegative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassInnerEffect {
    color: GlassColor,
    offset: [f32; 2],
    blur: f32,
    spread: f32,
}

impl GlassInnerEffect {
    /// Creates an effect with finite offsets and spread, and finite nonnegative blur.
    pub fn new(
        color: GlassColor,
        offset: [f32; 2],
        blur: f32,
        spread: f32,
    ) -> Result<Self, GlassStyleError> {
        for value in offset {
            if !value.is_finite() {
                return Err(GlassStyleError::NonFinite("effect offset"));
            }
        }
        Ok(Self {
            color,
            offset,
            blur: nonnegative(blur, "effect blur")?,
            spread: if spread.is_finite() {
                spread
            } else {
                return Err(GlassStyleError::NonFinite("effect spread"));
            },
        })
    }

    pub fn color(self) -> GlassColor {
        self.color
    }
    pub fn offset(self) -> [f32; 2] {
        self.offset
    }
    pub fn blur(self) -> f32 {
        self.blur
    }
    pub fn spread(self) -> f32 {
        self.spread
    }
}

#[derive(Clone)]
pub enum GlassValue<T> {
    Static(T),
    Reactive(Reactive<T>),
}

impl<T> From<T> for GlassValue<T> {
    fn from(value: T) -> Self {
        Self::Static(value)
    }
}

impl<T> From<Reactive<T>> for GlassValue<T> {
    fn from(value: Reactive<T>) -> Self {
        Self::Reactive(value)
    }
}

impl<T: Clone + 'static> GlassValue<T> {
    pub fn snapshot<C: AppContext>(&self, cx: &C) -> T {
        match self {
            Self::Static(value) => value.clone(),
            Self::Reactive(value) => value.get(cx),
        }
    }
}

impl<T> GlassValue<T> {
    pub fn as_static(&self) -> Option<&T> {
        match self {
            Self::Static(value) => Some(value),
            Self::Reactive(_) => None,
        }
    }

    pub fn as_reactive(&self) -> Option<&Reactive<T>> {
        match self {
            Self::Static(_) => None,
            Self::Reactive(value) => Some(value),
        }
    }
}

/// A partial style override. Unset properties inherit; optional effects can be explicitly disabled.
#[derive(Clone, Default)]
pub struct GlassPatch {
    radius: Option<GlassValue<GlassRadius>>,
    opacity: Option<GlassValue<GlassOpacity>>,
    tint: Option<GlassValue<GlassColor>>,
    border: Option<GlassValue<Option<GlassBorder>>>,
    rim: Option<GlassValue<Option<GlassRim>>>,
    inner_highlight: Option<GlassValue<Option<GlassInnerEffect>>>,
    inner_shadow: Option<GlassValue<Option<GlassInnerEffect>>>,
}

macro_rules! patch_builder {
    ($name:ident, $field:ident $(.$subfield:ident)?, $value:ty) => {
        pub fn $name(mut self, value: impl Into<GlassValue<$value>>) -> Self {
            self.$field $(.$subfield)? = Some(value.into());
            self
        }
    };
}

impl GlassPatch {
    patch_builder!(with_radius, radius, GlassRadius);
    patch_builder!(with_opacity, opacity, GlassOpacity);
    patch_builder!(with_tint, tint, GlassColor);
    patch_builder!(with_border, border, Option<GlassBorder>);
    patch_builder!(with_rim, rim, Option<GlassRim>);
    patch_builder!(
        with_inner_highlight,
        inner_highlight,
        Option<GlassInnerEffect>
    );
    patch_builder!(with_inner_shadow, inner_shadow, Option<GlassInnerEffect>);
}

/// Immutable, cloneable base Glass style with optional interaction-state overrides.
#[derive(Clone)]
pub struct GlassStyle {
    base: GlassPatch,
    hover: GlassPatch,
    pressed: GlassPatch,
    focus: GlassPatch,
}

impl Default for GlassStyle {
    fn default() -> Self {
        Self {
            base: GlassPatch::default()
                .with_radius(GlassRadius(12.0))
                .with_opacity(GlassOpacity(1.0))
                .with_tint(GlassColor {
                    red: 0.0,
                    green: 0.0,
                    blue: 0.0,
                    alpha: 0.25,
                }),
            hover: GlassPatch::default(),
            pressed: GlassPatch::default(),
            focus: GlassPatch::default(),
        }
    }
}

impl GlassStyle {
    patch_builder!(with_radius, base.radius, GlassRadius);
    patch_builder!(with_opacity, base.opacity, GlassOpacity);
    patch_builder!(with_tint, base.tint, GlassColor);
    patch_builder!(with_border, base.border, Option<GlassBorder>);
    patch_builder!(with_rim, base.rim, Option<GlassRim>);
    patch_builder!(
        with_inner_highlight,
        base.inner_highlight,
        Option<GlassInnerEffect>
    );
    patch_builder!(
        with_inner_shadow,
        base.inner_shadow,
        Option<GlassInnerEffect>
    );

    pub fn with_hover(mut self, patch: GlassPatch) -> Self {
        self.hover = patch;
        self
    }
    pub fn with_pressed(mut self, patch: GlassPatch) -> Self {
        self.pressed = patch;
        self
    }
    pub fn with_focus(mut self, patch: GlassPatch) -> Self {
        self.focus = patch;
        self
    }

    pub fn resolve<C: AppContext>(&self, state: GlassState, cx: &C) -> ResolvedGlassStyle {
        let mut result = ResolvedGlassStyle::default();
        result.apply(&self.base, cx);
        if state.hovered {
            result.apply(&self.hover, cx);
        }
        if state.pressed {
            result.apply(&self.pressed, cx);
        }
        if state.focused {
            result.apply(&self.focus, cx);
        }
        result
    }

    pub(crate) fn observe<Owner: 'static>(
        &self,
        cx: &mut Context<Owner>,
        callback: impl Fn(&mut Owner, &mut Context<Owner>) + Clone + 'static,
    ) -> Vec<ReactiveSubscription> {
        let mut subscriptions = Vec::new();
        for patch in [&self.base, &self.hover, &self.pressed, &self.focus] {
            observe_patch(patch, cx, callback.clone(), &mut subscriptions);
        }
        subscriptions
    }
}

fn observe_value<T: Clone + 'static, Owner: 'static>(
    value: &Option<GlassValue<T>>,
    cx: &mut Context<Owner>,
    callback: impl Fn(&mut Owner, &mut Context<Owner>) + 'static,
    subscriptions: &mut Vec<ReactiveSubscription>,
) {
    if let Some(GlassValue::Reactive(value)) = value {
        subscriptions.push(value.observe(cx, move |owner, _, cx| callback(owner, cx)));
    }
}

fn observe_patch<Owner: 'static>(
    patch: &GlassPatch,
    cx: &mut Context<Owner>,
    callback: impl Fn(&mut Owner, &mut Context<Owner>) + Clone + 'static,
    subscriptions: &mut Vec<ReactiveSubscription>,
) {
    observe_value(&patch.radius, cx, callback.clone(), subscriptions);
    observe_value(&patch.opacity, cx, callback.clone(), subscriptions);
    observe_value(&patch.tint, cx, callback.clone(), subscriptions);
    observe_value(&patch.border, cx, callback.clone(), subscriptions);
    observe_value(&patch.rim, cx, callback.clone(), subscriptions);
    observe_value(&patch.inner_highlight, cx, callback.clone(), subscriptions);
    observe_value(&patch.inner_shadow, cx, callback, subscriptions);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlassState {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedGlassStyle {
    pub radius: GlassRadius,
    pub opacity: GlassOpacity,
    pub tint: GlassColor,
    pub border: Option<GlassBorder>,
    pub rim: Option<GlassRim>,
    pub inner_highlight: Option<GlassInnerEffect>,
    pub inner_shadow: Option<GlassInnerEffect>,
}

impl Default for ResolvedGlassStyle {
    fn default() -> Self {
        Self {
            radius: GlassRadius(12.0),
            opacity: GlassOpacity(1.0),
            tint: GlassColor {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha: 0.25,
            },
            border: None,
            rim: None,
            inner_highlight: None,
            inner_shadow: None,
        }
    }
}

impl ResolvedGlassStyle {
    fn apply<C: AppContext>(&mut self, patch: &GlassPatch, cx: &C) {
        if let Some(value) = &patch.radius {
            self.radius = value.snapshot(cx);
        }
        if let Some(value) = &patch.opacity {
            self.opacity = value.snapshot(cx);
        }
        if let Some(value) = &patch.tint {
            self.tint = value.snapshot(cx);
        }
        if let Some(value) = &patch.border {
            self.border = value.snapshot(cx);
        }
        if let Some(value) = &patch.rim {
            self.rim = value.snapshot(cx);
        }
        if let Some(value) = &patch.inner_highlight {
            self.inner_highlight = value.snapshot(cx);
        }
        if let Some(value) = &patch.inner_shadow {
            self.inner_shadow = value.snapshot(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::State;
    use gpui::{App, AppContext, TestAppContext};

    fn color() -> GlassColor {
        GlassColor::rgba(1.0, 0.5, 0.25, 1.0).unwrap()
    }

    fn assert_invalid_rgba(channel: usize, value: f32) {
        let mut rgba = [0.5; 4];
        rgba[channel] = value;
        assert!(GlassColor::rgba(rgba[0], rgba[1], rgba[2], rgba[3]).is_err());
    }

    #[test]
    fn constructors_validate_every_channel_and_effect_parameter() {
        let nonfinite = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        for invalid in nonfinite {
            assert!(GlassRadius::new(invalid).is_err());
            assert!(GlassOpacity::new(invalid).is_err());
            assert!(GlassBorder::new(invalid, color()).is_err());
            assert!(GlassRim::new(invalid, 0.0, color()).is_err());
            assert!(GlassRim::new(0.0, invalid, color()).is_err());
            assert!(GlassInnerEffect::new(color(), [invalid, 0.0], 0.0, 0.0).is_err());
            assert!(GlassInnerEffect::new(color(), [0.0, invalid], 0.0, 0.0).is_err());
            assert!(GlassInnerEffect::new(color(), [0.0, 0.0], invalid, 0.0).is_err());
            assert!(GlassInnerEffect::new(color(), [0.0, 0.0], 0.0, invalid).is_err());
            for channel in 0..4 {
                assert_invalid_rgba(channel, invalid);
            }
        }
        for negative in [-1.0, -f32::EPSILON] {
            assert!(GlassRadius::new(negative).is_err());
            assert!(GlassBorder::new(negative, color()).is_err());
            assert!(GlassRim::new(negative, 0.0, color()).is_err());
            assert!(GlassRim::new(0.0, negative, color()).is_err());
            assert!(GlassInnerEffect::new(color(), [0.0, 0.0], negative, 0.0).is_err());
            assert!(GlassInnerEffect::new(color(), [0.0, 0.0], 0.0, negative).is_ok());
            for channel in 0..4 {
                assert_invalid_rgba(channel, negative);
            }
        }
        for above_one in [1.0 + f32::EPSILON, 2.0] {
            assert!(GlassOpacity::new(above_one).is_err());
            for channel in 0..4 {
                assert_invalid_rgba(channel, above_one);
            }
        }

        assert_eq!(GlassRadius::new(0.0).unwrap().get(), 0.0);
        assert_eq!(GlassOpacity::new(0.0).unwrap().get(), 0.0);
        assert_eq!(
            GlassColor::rgba(0.0, 0.0, 0.0, 0.0).unwrap(),
            GlassColor {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha: 0.0,
            }
        );
        assert_eq!(GlassBorder::new(0.0, color()).unwrap().width(), 0.0);
        assert_eq!(GlassRim::new(0.0, 0.0, color()).unwrap().inset(), 0.0);
        let effect = GlassInnerEffect::new(color(), [0.0, 0.0], 0.0, -2.0).unwrap();
        assert_eq!(effect.spread(), -2.0);
        assert!(GlassInnerEffect::new(color(), [-1.0, 2.0], 0.0, 0.0).is_ok());
    }

    #[test]
    fn clone_defaults_and_all_state_combinations_follow_sequential_merge() {
        let cx = TestAppContext::single();
        let default = GlassStyle::default();
        let default_clone = default.clone();
        let changed = default.clone().with_radius(GlassRadius::new(30.0).unwrap());
        assert_eq!(
            default.resolve(GlassState::default(), &cx).radius.get(),
            12.0
        );
        assert_eq!(
            default_clone
                .resolve(GlassState::default(), &cx)
                .radius
                .get(),
            12.0
        );
        assert_eq!(
            changed.resolve(GlassState::default(), &cx).radius.get(),
            30.0
        );
        assert_eq!(
            default.resolve(
                GlassState {
                    hovered: true,
                    pressed: true,
                    focused: true
                },
                &cx
            ),
            default.resolve(GlassState::default(), &cx),
        );

        let style = default
            .with_radius(GlassRadius::new(1.0).unwrap())
            .with_hover(GlassPatch::default().with_radius(GlassRadius::new(2.0).unwrap()))
            .with_pressed(GlassPatch::default().with_radius(GlassRadius::new(3.0).unwrap()))
            .with_focus(GlassPatch::default().with_radius(GlassRadius::new(4.0).unwrap()));
        for flags in 0..8 {
            let state = GlassState {
                hovered: flags & 1 != 0,
                pressed: flags & 2 != 0,
                focused: flags & 4 != 0,
            };
            let expected = if state.focused {
                4.0
            } else if state.pressed {
                3.0
            } else if state.hovered {
                2.0
            } else {
                1.0
            };
            assert_eq!(style.resolve(state, &cx).radius.get(), expected);
        }
    }

    #[test]
    fn partial_overrides_inherit_and_explicit_none_disables_optional_effects() {
        let cx = TestAppContext::single();
        let border = GlassBorder::new(1.0, color()).unwrap();
        let rim = GlassRim::new(1.0, 2.0, color()).unwrap();
        let style = GlassStyle::default()
            .with_border(Some(border))
            .with_rim(Some(rim))
            .with_hover(GlassPatch::default().with_border(None))
            .with_pressed(GlassPatch::default().with_opacity(GlassOpacity::new(0.6).unwrap()))
            .with_focus(GlassPatch::default().with_radius(GlassRadius::new(4.0).unwrap()));
        let resolved = style.resolve(
            GlassState {
                hovered: true,
                pressed: true,
                focused: true,
            },
            &cx,
        );
        assert_eq!(resolved.border, None);
        assert_eq!(resolved.rim, Some(rim));
        assert_eq!(resolved.opacity.get(), 0.6);
        assert_eq!(resolved.radius.get(), 4.0);
        assert_eq!(resolved.tint.alpha(), 0.25);
    }

    fn set_opacity(
        state: &State<GlassOpacity>,
        value: f32,
        cx: &mut App,
    ) -> Result<(), GlassStyleError> {
        state.set(GlassOpacity::new(value)?, cx);
        Ok(())
    }

    #[test]
    fn invalid_value_is_rejected_before_state_publication() {
        let cx = TestAppContext::single();
        let (state, owner, subscriptions) = cx.update(|cx| {
            let state = State::new(GlassOpacity::new(0.5).unwrap(), cx);
            let owner = cx.new(|_| 0usize);
            let style = GlassStyle::default().with_opacity(state.reactive());
            let subscriptions = owner.update(cx, |_, owner_cx| {
                style.observe(owner_cx, |count, _| *count += 1)
            });
            (state, owner, subscriptions)
        });
        let result = cx.update(|cx| set_opacity(&state, f32::NAN, cx));
        assert!(result.is_err());
        assert_eq!(state.reactive().get(&cx).get(), 0.5);
        assert_eq!(owner.read_with(&cx, |count, _| *count), 0);
        drop(subscriptions);
    }

    #[test]
    fn reactive_base_and_patch_properties_update_and_observers_are_retained() {
        let cx = TestAppContext::single();
        let (states, style, owner, subscriptions) = cx.update(|cx| {
            let radius = State::new(GlassRadius::new(8.0).unwrap(), cx);
            let opacity = State::new(GlassOpacity::new(0.5).unwrap(), cx);
            let tint = State::new(color(), cx);
            let border = State::new(Some(GlassBorder::new(1.0, color()).unwrap()), cx);
            let rim = State::new(Some(GlassRim::new(1.0, 1.0, color()).unwrap()), cx);
            let highlight = State::new(
                Some(GlassInnerEffect::new(color(), [0.0, 0.0], 2.0, -1.0).unwrap()),
                cx,
            );
            let shadow = State::new(
                Some(GlassInnerEffect::new(color(), [1.0, -1.0], 2.0, 1.0).unwrap()),
                cx,
            );
            let patch_radius = State::new(GlassRadius::new(9.0).unwrap(), cx);
            let style = GlassStyle::default()
                .with_radius(radius.reactive())
                .with_opacity(opacity.reactive())
                .with_tint(tint.reactive())
                .with_border(border.reactive())
                .with_rim(rim.reactive())
                .with_inner_highlight(highlight.reactive())
                .with_inner_shadow(shadow.reactive())
                .with_hover(GlassPatch::default().with_radius(patch_radius.reactive()));
            let owner = cx.new(|_| 0usize);
            let subscriptions = owner.update(cx, |_, owner_cx| {
                style.observe(owner_cx, |count, _| *count += 1)
            });
            (
                (
                    radius,
                    opacity,
                    tint,
                    border,
                    rim,
                    highlight,
                    shadow,
                    patch_radius,
                ),
                style,
                owner,
                subscriptions,
            )
        });

        let (radius, opacity, tint, border, rim, highlight, shadow, patch_radius) = states;
        cx.update(|cx| {
            radius.set(GlassRadius::new(7.0).unwrap(), cx);
            opacity.set(GlassOpacity::new(0.25).unwrap(), cx);
            tint.set(GlassColor::rgba(0.1, 0.2, 0.3, 0.4).unwrap(), cx);
            border.set(None, cx);
            rim.set(None, cx);
            highlight.set(None, cx);
            shadow.set(None, cx);
            patch_radius.set(GlassRadius::new(6.0).unwrap(), cx);
        });
        let resolved = style.resolve(
            GlassState {
                hovered: true,
                ..GlassState::default()
            },
            &cx,
        );
        assert_eq!(resolved.radius.get(), 6.0);
        assert_eq!(resolved.opacity.get(), 0.25);
        assert_eq!(resolved.tint, GlassColor::rgba(0.1, 0.2, 0.3, 0.4).unwrap());
        assert_eq!(resolved.border, None);
        assert_eq!(resolved.rim, None);
        assert_eq!(resolved.inner_highlight, None);
        assert_eq!(resolved.inner_shadow, None);
        assert_eq!(owner.read_with(&cx, |count, _| *count), 8);
        drop(subscriptions);
    }

    #[test]
    fn reactive_map_and_combine_keep_values_validated() {
        let cx = TestAppContext::single();
        let (first, second, combined, style) = cx.update(|cx| {
            let first = State::new(GlassOpacity::new(0.2).unwrap(), cx);
            let second = State::new(GlassRadius::new(1.0).unwrap(), cx);
            let mapped = first.reactive().map(cx, |opacity| {
                GlassRadius::new(opacity.get() * 10.0).unwrap()
            });
            let combined = mapped.combine(&second.reactive(), cx, |left, right| {
                GlassRadius::new(left.get() + right.get()).unwrap()
            });
            let style = GlassStyle::default().with_radius(combined.clone());
            (first, second, combined, style)
        });
        assert_eq!(style.resolve(GlassState::default(), &cx).radius.get(), 3.0);
        cx.update(|cx| {
            first.set(GlassOpacity::new(0.4).unwrap(), cx);
            second.set(GlassRadius::new(2.0).unwrap(), cx);
        });
        assert_eq!(combined.get(&cx).get(), 6.0);
        assert_eq!(style.resolve(GlassState::default(), &cx).radius.get(), 6.0);
    }
}

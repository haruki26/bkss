use std::time::Duration;

use super::PlanTiming;
use super::spring::spring_state;

const SETTLE_EPSILON: f64 = 0.001;

pub(super) fn finish_elapsed(
    origin: (f64, f64),
    target: f64,
    span: f64,
    timing: Option<PlanTiming>,
) -> Duration {
    let Some(timing) = timing else {
        return Duration::ZERO;
    };
    match timing {
        PlanTiming::Easing { duration, .. } => {
            if origin.0 == target {
                Duration::ZERO
            } else {
                duration
            }
        }
        PlanTiming::Spring {
            stiffness,
            damping,
            mass,
            max_duration,
        } => {
            let y = origin.0 - target;
            let initial_position = y.abs() / span;
            let initial_velocity = origin.1.abs() / span;
            if initial_position < SETTLE_EPSILON && initial_velocity < SETTLE_EPSILON {
                // A moving zero-distance state is ineligible at t=0, even below tolerance.
                return if y == 0.0 && origin.1 != 0.0 {
                    Duration::from_nanos(1)
                } else {
                    Duration::ZERO
                };
            }
            let physics = [stiffness as f64, damping as f64, mass as f64];
            let max_nanos = max_duration.as_nanos();
            if !settled_by(physics, origin, target, span, max_duration.as_secs_f64()) {
                return max_duration;
            }
            let mut low = 0_u128;
            let mut high = max_nanos;
            while high - low > 1 {
                let middle = low + (high - low) / 2;
                let seconds = duration_from_nanos(middle).as_secs_f64();
                if settled_by(physics, origin, target, span, seconds) {
                    high = middle;
                } else {
                    low = middle;
                }
            }
            duration_from_nanos(high)
        }
    }
}

fn duration_from_nanos(nanos: u128) -> Duration {
    Duration::new(
        (nanos / 1_000_000_000) as u64,
        (nanos % 1_000_000_000) as u32,
    )
}

fn settled_by(physics: [f64; 3], origin: (f64, f64), target: f64, span: f64, until: f64) -> bool {
    let settled = |time: f64| {
        let (position, velocity) = spring_state(physics, origin.0, origin.1, target, time);
        (position - target).abs() / span < SETTLE_EPSILON && velocity.abs() / span < SETTLE_EPSILON
    };
    if settled(0.0) {
        return true;
    }
    let [stiffness, damping, mass] = physics;
    let y = origin.0 - target;
    let alpha = damping / (2.0 * mass);
    let omega_squared = stiffness / mass;
    let discriminant = damping * damping - 4.0 * stiffness * mass;
    if discriminant < 0.0 {
        let beta = (-discriminant).sqrt() / (2.0 * mass);
        let b = (origin.1 + alpha * y) / beta;
        let y_coeff = (y, b);
        let v_coeff = (origin.1, -(beta * y + alpha * b));
        let a_coeff = (
            -2.0 * alpha * v_coeff.0 - omega_squared * y_coeff.0,
            -2.0 * alpha * v_coeff.1 - omega_squared * y_coeff.1,
        );
        for coefficients in [
            v_coeff,
            a_coeff,
            subtract(y_coeff, v_coeff),
            add(y_coeff, v_coeff),
        ] {
            if under_family_below(
                coefficients,
                (beta, alpha),
                physics,
                origin,
                target,
                span,
                until,
            ) {
                return true;
            }
        }
    } else if discriminant == 0.0 {
        let c = origin.1 + alpha * y;
        for (a, b) in [
            (origin.1, -alpha * c),
            (-alpha * origin.1 - alpha * c, alpha * alpha * c),
            (y - origin.1, c + alpha * c),
            (y + origin.1, c - alpha * c),
        ] {
            if let Some(time) = linear_root(a, b)
                && time <= until
                && settled(time)
            {
                return true;
            }
        }
    } else {
        let root = discriminant.sqrt();
        let slow = -(2.0 * stiffness) / (damping + root);
        let fast = -(damping + root) / (2.0 * mass);
        let denominator = slow - fast;
        let a = (origin.1 - fast * y) / denominator;
        let b = (slow * y - origin.1) / denominator;
        let y_coeff = (a, b);
        let v_coeff = (a * slow, b * fast);
        let acceleration = (a * slow * slow, b * fast * fast);
        for (c1, c2) in [
            v_coeff,
            acceleration,
            subtract(y_coeff, v_coeff),
            add(y_coeff, v_coeff),
        ] {
            if let Some(time) = exponential_root(c1, c2, slow, fast)
                && time <= until
                && settled(time)
            {
                return true;
            }
        }
    }
    settled(until)
}

fn add(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 + b.0, a.1 + b.1)
}
fn subtract(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 - b.0, a.1 - b.1)
}

fn linear_root(constant: f64, slope: f64) -> Option<f64> {
    if slope == 0.0 {
        None
    } else {
        let time = -constant / slope;
        (time >= 0.0 && time.is_finite()).then_some(time)
    }
}

fn exponential_root(a: f64, b: f64, slow: f64, fast: f64) -> Option<f64> {
    if a == 0.0 || b == 0.0 {
        return None;
    }
    let ratio = -b / a;
    if ratio <= 0.0 || !ratio.is_finite() {
        return None;
    }
    let time = ratio.ln() / (slow - fast);
    (time >= 0.0 && time.is_finite()).then_some(time)
}

fn under_family_below(
    coefficients: (f64, f64),
    (beta, alpha): (f64, f64),
    physics: [f64; 3],
    origin: (f64, f64),
    target: f64,
    span: f64,
    until: f64,
) -> bool {
    let (a, b) = coefficients;
    if a == 0.0 && b == 0.0 {
        return false;
    }
    let mut phase = (-a).atan2(b).rem_euclid(std::f64::consts::PI);
    if phase == 0.0 {
        phase = std::f64::consts::PI;
    }
    let first = phase / beta;
    let half_period = std::f64::consts::PI / beta;
    if first > until {
        return false;
    }
    let n = ((until - first) / half_period).floor();
    let decay_duration = n * half_period;
    let (position, velocity) = spring_state(physics, origin.0, origin.1, target, first);
    let base = ((position - target).abs().max(velocity.abs()) / span).ln();
    base - alpha * decay_duration < SETTLE_EPSILON.ln() && n >= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spring(k: f32, c: f32, m: f32, max_ms: u64) -> Option<PlanTiming> {
        Some(PlanTiming::Spring {
            stiffness: k,
            damping: c,
            mass: m,
            max_duration: Duration::from_millis(max_ms),
        })
    }

    #[test]
    fn decayed_large_inherited_velocity_remains_unsettled_until_threshold() {
        let physics = [100.0, 20.0, 1.0];
        let origin = (0.0, 1e40);
        let time = 10.2;
        assert!(!settled_by(physics, origin, 0.0, 1.0, time));
        let finish = finish_elapsed(origin, 0.0, 1.0, spring(100.0, 20.0, 1.0, 20_000));
        assert!(finish > Duration::from_secs_f64(time));
        assert!(!settled_by([100.0, 4.0, 1.0], origin, 0.0, 1.0, time));
    }

    #[test]
    fn zero_distance_motion_gets_positive_time_but_stationary_micro_error_snaps() {
        let timing = spring(100.0, 10.0, 1.0, 2000);
        let finish = finish_elapsed((1.0, 2.0), 1.0, 1.0, timing);
        assert!(finish > Duration::from_millis(1));
        let state = spring_state([100.0, 10.0, 1.0], 1.0, 2.0, 1.0, finish.as_secs_f64());
        assert!((state.0 - 1.0).abs() < SETTLE_EPSILON);
        assert!(state.1.abs() < SETTLE_EPSILON);
        assert_eq!(
            finish_elapsed((1.0, 0.0005), 1.0, 1.0, timing),
            Duration::from_nanos(1)
        );
        assert_eq!(
            finish_elapsed((1.0005, 0.0), 1.0, 1.0, timing),
            Duration::ZERO
        );
        assert_eq!(finish_elapsed((1.0, 0.0), 1.0, 1.0, timing), Duration::ZERO);
    }

    #[test]
    fn first_threshold_entry_survives_later_high_frequency_transient_excursion() {
        let physics = [1e6, 0.002, 1.0];
        let origin = (0.0, 0.0);
        let target = 1.0;
        let span = 1.0;
        let max = Duration::from_secs(20_000);
        let timing = Some(PlanTiming::Spring {
            stiffness: 1e6_f32,
            damping: 0.002,
            mass: 1.0,
            max_duration: max,
        });
        let finish = finish_elapsed(origin, target, span, timing);
        assert!(finish > Duration::ZERO && finish < max, "{finish:?}");
        assert!(!settled_by(
            physics,
            origin,
            target,
            span,
            (finish - Duration::from_nanos(1)).as_secs_f64()
        ));
        let half_period = std::f64::consts::PI / (1e6_f64 - 0.001_f64 * 0.001_f64).sqrt();
        let transient = (1..=8).any(|i| {
            let t = finish.as_secs_f64() + half_period * i as f64 / 8.0;
            let (position, velocity) = spring_state(physics, origin.0, origin.1, target, t);
            (position - target).abs().max(velocity.abs()) >= SETTLE_EPSILON
                && settled_by(physics, origin, target, span, t)
        });
        assert!(transient, "finish={finish:?}");
    }
}

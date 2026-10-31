pub(super) fn spring_state(
    physics: [f64; 3],
    position: f64,
    velocity: f64,
    target: f64,
    t: f64,
) -> (f64, f64) {
    if t == 0.0 {
        return (position, velocity);
    }
    let [stiffness, damping, mass] = physics;
    let y = position - target;
    let alpha = damping / (2.0 * mass);
    let omega_squared = stiffness / mass;
    let discriminant = damping * damping - 4.0 * stiffness * mass;
    let u = alpha * t;
    let decay = (-u).exp();

    if discriminant == 0.0 {
        let expm1 = (-u).exp_m1();
        let a = if u.abs() < 1e-3 {
            small_one_minus_f(omega_squared, alpha, t)
        } else {
            -(expm1 + decay * u)
        };
        let f = decay * (1.0 + u);
        let b = decay * t;
        let d = decay * (1.0 - u);
        return (
            anchored_position(position, target, y, velocity, a, f, b),
            -omega_squared * y * b + velocity * d,
        );
    }

    if discriminant < 0.0 {
        let beta = (-discriminant).sqrt() / (2.0 * mass);
        let z = beta * t;
        let sinc = sinc(z);
        let expm1 = (-u).exp_m1();
        let cos_minus_one = -2.0 * (z * 0.5).sin().powi(2);
        let a = if u.abs().max(z.abs()) < 1e-3 {
            small_one_minus_f(omega_squared, alpha, t)
        } else {
            -(expm1 + decay * (cos_minus_one + alpha * t * sinc))
        };
        let f = decay * (z.cos() + alpha * t * sinc);
        let b = decay * t * sinc;
        let d = decay * (z.cos() - alpha * t * sinc);
        return (
            anchored_position(position, target, y, velocity, a, f, b),
            -omega_squared * y * b + velocity * d,
        );
    }

    let beta = discriminant.sqrt() / (2.0 * mass);
    let z = beta * t;
    if z.abs() < 0.5 {
        let cosh_minus_one = 2.0 * (z * 0.5).sinh().powi(2);
        let sinhc = sinhc(z);
        let expm1 = (-u).exp_m1();
        let a = if u.abs().max(z.abs()) < 1e-3 {
            small_one_minus_f(omega_squared, alpha, t)
        } else {
            -(expm1 + decay * (cosh_minus_one + alpha * t * sinhc))
        };
        let f = decay * (z.cosh() + alpha * t * sinhc);
        let b = decay * t * sinhc;
        let d = decay * (z.cosh() - alpha * t * sinhc);
        return (
            anchored_position(position, target, y, velocity, a, f, b),
            -omega_squared * y * b + velocity * d,
        );
    }

    let slow = -(2.0 * stiffness) / (damping + discriminant.sqrt());
    let fast = -(damping + discriminant.sqrt()) / (2.0 * mass);
    let denominator = slow - fast;
    let slow_exp = (slow * t).exp();
    let fast_exp = (fast * t).exp();
    let separation = (fast - slow) * t;
    let b = slow_exp * -separation.exp_m1() / denominator;
    let f = (-fast * slow_exp + slow * fast_exp) / denominator;
    let a = (fast * (slow * t).exp_m1() - slow * (fast * t).exp_m1()) / denominator;
    let d = (slow * slow_exp - fast * fast_exp) / denominator;
    (
        anchored_position(position, target, y, velocity, a, f, b),
        -omega_squared * y * b + velocity * d,
    )
}

fn small_one_minus_f(omega_squared: f64, alpha: f64, t: f64) -> f64 {
    let omega_t_squared = omega_squared * t * t;
    let alpha_t = alpha * t;
    omega_t_squared / 2.0 - alpha_t * omega_t_squared / 3.0
        + (4.0 * alpha_t * alpha_t - omega_t_squared) * omega_t_squared / 24.0
}

fn anchored_position(
    position: f64,
    target: f64,
    displacement: f64,
    velocity: f64,
    one_minus_f: f64,
    f: f64,
    b: f64,
) -> f64 {
    if one_minus_f > 0.5 {
        target + displacement * f + velocity * b
    } else {
        position - displacement * one_minus_f + velocity * b
    }
}

fn sinc(value: f64) -> f64 {
    if value.abs() < 1e-4 {
        let square = value * value;
        1.0 - square / 6.0 + square * square / 120.0
    } else {
        value.sin() / value
    }
}

fn sinhc(value: f64) -> f64 {
    if value.abs() < 1e-4 {
        let square = value * value;
        1.0 + square / 6.0 + square * square / 120.0
    } else {
        value.sinh() / value
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn large_z_overdamped_position_keeps_the_small_slow_mode_increment() {
        let target = f32::MAX as f64;
        let physics = [1.0, 1e30, 1.0];
        let mut previous = (1.0, 0.0);
        for nanos in [0, 1, 2] {
            let time = Duration::from_nanos(nanos);
            let state = spring_state(physics, 1.0, 0.0, target, time.as_secs_f64());
            if nanos == 0 {
                assert_eq!(state, (1.0, 0.0));
            } else {
                let slow = -2.0 / (1e30 + (1e60_f64 - 4.0).sqrt());
                let expected = 1.0 + (target - 1.0) * -((slow * time.as_secs_f64()).exp_m1());
                assert!(
                    (state.0 - expected).abs() < 1e-12,
                    "{nanos}ns: {state:?}, expected {expected}"
                );
                assert!(
                    state.0 > previous.0,
                    "{nanos}ns: {state:?} after {previous:?}"
                );
            }
            previous = state;
        }
    }

    #[test]
    fn critical_decayed_large_velocity_is_evaluated_without_cancellation() {
        let physics = [100.0, 20.0, 1.0];
        let t = 10.2;
        let decay = (-10.0_f64 * t).exp();
        let expected = (1e40 * decay * t, 1e40 * decay * (1.0 - 10.0 * t));
        let state = spring_state(physics, 0.0, 1e40, 0.0, t);
        assert!((state.0 - expected.0).abs() < 1e-15);
        assert!((state.1 - expected.1).abs() < 1e-15);
    }

    #[test]
    fn underdamped_decayed_large_velocity_is_evaluated_without_cancellation() {
        let physics = [100.0, 4.0, 1.0];
        let t = 10.2;
        let alpha = 2.0;
        let beta = (100.0_f64 - alpha * alpha).sqrt();
        let z = beta * t;
        let decay = (-alpha * t).exp();
        let expected = (
            1e40 * decay * t * sinc(z),
            1e40 * decay * (z.cos() - alpha * t * sinc(z)),
        );
        let state = spring_state(physics, 0.0, 1e40, 0.0, t);
        assert!((state.0 - expected.0).abs() / expected.0.abs() < 1e-14);
        assert!((state.1 - expected.1).abs() / expected.1.abs() < 1e-14);
    }

    #[test]
    fn critical_near_critical_and_all_damping_regimes_are_stable() {
        let critical = spring_state([100.0, 20.0, 1.0], 0.0, 0.0, 1.0, 0.2);
        for [k, c, m] in [[100.0, 4.0, 1.0], [100.0, 20.0, 1.0], [100.0, 40.0, 1.0]] {
            let state = spring_state([k, c, m], 0.0, 0.0, 1.0, 0.5);
            assert!(state.0.is_finite() && state.1.is_finite());
        }
        for damping in [19.999999, 20.000001] {
            let near = spring_state([100.0, damping, 1.0], 0.0, 0.0, 1.0, 0.2);
            assert!((near.0 - critical.0).abs() < 1e-6, "{near:?} {critical:?}");
            assert!((near.1 - critical.1).abs() < 1e-5, "{near:?} {critical:?}");
        }
    }
}

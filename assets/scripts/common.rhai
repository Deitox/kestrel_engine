// Shared helpers for Rhai behaviours. Import with:
// import "common" as common;

fn clamp01(x) {
    if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

fn lerp(a, b, t) {
    let k = clamp01(t);
    a + (b - a) * k
}

fn atan2_safe(y, x) {
    let pi = 3.141592653589793;
    let eps = 0.000001;
    if x.abs() > eps {
        let base = (y / x).atan();
        if x > 0.0 {
            base
        } else if y >= 0.0 {
            base + pi
        } else {
            base - pi
        }
    } else if y.abs() > eps {
        if y > 0.0 { pi * 0.5 } else { -pi * 0.5 }
    } else {
        0.0
    }
}

fn vec2(x, y) {
    [x, y]
}

fn vec2_dot(a, b) {
    (a[0] * b[0]) + (a[1] * b[1])
}

fn vec2_len(v) {
    (vec2_dot(v, v)).sqrt()
}

fn vec2_normalize(v) {
    let len = vec2_len(v);
    if len <= 0.0001 {
        [0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

fn vec2_distance(a, b) {
    vec2_len([b[0] - a[0], b[1] - a[1]])
}

fn vec2_lerp(a, b, t) {
    let k = clamp01(t);
    [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k]
}

fn move_toward(current, target, max_delta) {
    let delta = target - current;
    let step = if delta.abs() <= max_delta.abs() { delta } else { delta.sign() * max_delta.abs() };
    current + step
}

fn move_toward_vec2(current, target, max_delta) {
    let cx = current[0];
    let cy = current[1];
    let dx = target[0] - cx;
    let dy = target[1] - cy;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist <= max_delta || dist <= 0.0001 {
        [target[0], target[1]]
    } else {
        let k = max_delta / dist;
        [cx + dx * k, cy + dy * k]
    }
}

fn clamp_length(v, max_len) {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len <= max_len || len <= 0.0001 {
        v
    } else {
        let k = max_len / len;
        [v[0] * k, v[1] * k]
    }
}

fn look_at(from, to) {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    atan2_safe(dy, dx)
}

fn angle_to_vec(angle) {
    [angle.cos(), angle.sin()]
}

fn vec_to_angle(v) {
    atan2_safe(v[1], v[0])
}

fn wrap_angle_pi(rad) {
    let pi = 3.141592653589793;
    let two_pi = 6.283185307179586;
    let a = rad;
    while a > pi { a -= two_pi; }
    while a < -pi { a += two_pi; }
    a
}

fn timer(seconds) {
    #{ duration: seconds.max(0.0), elapsed: 0.0 }
}

fn timer_tick(t, dt) {
    let out = t;
    if out.duration <= 0.0 {
        return #{ timer: out, fired: true };
    }
    out.elapsed += dt;
    let fired = if out.elapsed >= out.duration {
        out.elapsed -= out.duration;
        true
    } else {
        false
    };
    #{ timer: out, fired: fired }
}

fn cooldown(seconds) {
    #{ duration: seconds.max(0.0), remaining: 0.0 }
}

fn cooldown_ready(cd) {
    cd.remaining <= 0.0
}

fn cooldown_trigger(cd) {
    let out = cd;
    out.remaining = out.duration;
    out
}

fn cooldown_tick(cd, dt) {
    let out = cd;
    if out.remaining > 0.0 {
        out.remaining -= dt;
        if out.remaining < 0.0 {
            out.remaining = 0.0;
        }
    }
    out
}

fn smoothstep(t) {
    let k = clamp01(t);
    k * k * (3.0 - 2.0 * k)
}

fn deg_to_rad(deg) {
    deg * 0.017453292519943295
}

fn rad_to_deg(rad) {
    rad * 57.29577951308232
}

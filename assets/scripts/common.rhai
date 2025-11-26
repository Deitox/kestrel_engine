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
    dy.atan2(dx)
}

fn timer(seconds) {
    #{ duration: seconds.max(0.0), elapsed: 0.0 }
}

fn timer_tick(t, dt) {
    if t.duration <= 0.0 {
        return true;
    }
    t.elapsed += dt;
    if t.elapsed >= t.duration {
        t.elapsed -= t.duration;
        true
    } else {
        false
    }
}

fn cooldown(seconds) {
    #{ duration: seconds.max(0.0), remaining: 0.0 }
}

fn cooldown_ready(cd) {
    cd.remaining <= 0.0
}

fn cooldown_trigger(cd) {
    cd.remaining = cd.duration;
}

fn cooldown_tick(cd, dt) {
    if cd.remaining > 0.0 {
        cd.remaining -= dt;
        if cd.remaining < 0.0 {
            cd.remaining = 0.0;
        }
    }
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

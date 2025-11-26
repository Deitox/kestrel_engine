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

fn deg_to_rad(deg) {
    deg * 0.017453292519943295
}

fn rad_to_deg(rad) {
    rad * 57.29577951308232
}

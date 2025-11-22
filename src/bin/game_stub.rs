//! Minimal "game-only" binary that links `kestrel_engine` without pulling in Studio.
//! Intended as a placeholder target to verify engine-only builds remain viable.

fn main() {
    // Use a tiny piece of the engine API so the binary actually links the crate.
    let angle = kestrel_engine::wrap_angle(1.0);
    println!("kestrel_engine game stub (wrap_angle(1.0) = {angle})");
}

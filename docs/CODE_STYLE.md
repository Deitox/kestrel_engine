# Code Style & Build Checks

- **Formatting**: run `cargo fmt` (Rustfmt 1.7+) before commits. The default style is enforced via `rustfmt.toml`.
- **Linting**: run `cargo clippy --all-targets --all-features -D warnings` to catch regressions early.
- **Build**: `cargo check` for fast iteration; `cargo run --release` for performance profiling.
- **Imports**: group crate-local modules first, followed by external crates, then std. Use `crate::` paths for internal modules to make dependencies explicit.
- **Error handling**: prefer `anyhow::Context` for fallible initialization, log recoverable issues, and avoid `unwrap`/`expect` outside of hot paths with clear invariants.
- **Comments**: focus on explaining intent for non-obvious code paths (e.g., collision resolution, GPU setup). Avoid restating self-evident code.

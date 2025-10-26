use kestrel_engine::cli::CliOverrides;
use kestrel_engine::run_with_overrides;

fn main() {
    let cli_overrides = match CliOverrides::parse_from_env() {
        Ok(parsed) => parsed.into_config_overrides(),
        Err(err) => {
            eprintln!("[cli] {err}");
            std::process::exit(2);
        }
    };
    if let Err(err) = pollster::block_on(run_with_overrides(cli_overrides)) {
        eprintln!("Application error: {err:?}");
    }
}

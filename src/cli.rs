use crate::config::AppConfigOverrides;
use anyhow::{anyhow, bail, Context, Result};
use std::env;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliOverrides {
    width: Option<u32>,
    height: Option<u32>,
    vsync: Option<bool>,
}

impl CliOverrides {
    pub fn parse_from_env() -> Result<Self> {
        Self::parse(env::args())
    }

    pub fn parse<I, S>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut overrides = CliOverrides::default();
        let mut iter = args.into_iter();
        let _ = iter.next(); // skip program name if present
        while let Some(raw_flag) = iter.next() {
            let flag = raw_flag.as_ref();
            if !flag.starts_with("--") {
                bail!("Unexpected argument '{flag}'. Use --width/--height/--vsync with values.");
            }
            let key = &flag[2..];
            let value =
                iter.next().ok_or_else(|| anyhow!("Expected a value after '{flag}'"))?.as_ref().to_string();
            match key {
                "width" => {
                    overrides.width =
                        Some(value.parse::<u32>().with_context(|| format!("Invalid width '{value}'"))?);
                }
                "height" => {
                    overrides.height =
                        Some(value.parse::<u32>().with_context(|| format!("Invalid height '{value}'"))?);
                }
                "vsync" => {
                    overrides.vsync = Some(parse_bool_flag("vsync", &value)?);
                }
                _ => bail!("Unknown flag '{flag}'. Supported flags: --width, --height, --vsync."),
            }
        }
        Ok(overrides)
    }

    pub fn into_config_overrides(self) -> AppConfigOverrides {
        AppConfigOverrides { width: self.width, height: self.height, vsync: self.vsync }
    }

    #[cfg(test)]
    pub fn as_tuple(&self) -> (Option<u32>, Option<u32>, Option<bool>) {
        (self.width, self.height, self.vsync)
    }
}

fn parse_bool_flag(flag: &str, value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Ok(true),
        "0" | "false" | "off" | "no" => Ok(false),
        other => bail!("Invalid {flag} value '{other}'. Use on/off or true/false."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_width_height_and_vsync() {
        let args = ["app", "--width", "1600", "--height", "900", "--vsync", "off"];
        let overrides = CliOverrides::parse(args).expect("parse overrides");
        assert_eq!(overrides.as_tuple(), (Some(1600), Some(900), Some(false)));
    }

    #[test]
    fn latest_flag_wins() {
        let args = ["app", "--width", "800", "--width", "1920", "--vsync", "on", "--vsync", "off"];
        let overrides = CliOverrides::parse(args).expect("parse overrides");
        assert_eq!(overrides.as_tuple(), (Some(1920), None, Some(false)));
    }

    #[test]
    fn missing_value_errors() {
        let err = CliOverrides::parse(["app", "--width"]).unwrap_err();
        assert!(err.to_string().contains("Expected a value"), "error should mention missing value");
    }

    #[test]
    fn rejects_unknown_flags() {
        let err = CliOverrides::parse(["app", "--foo", "bar"]).unwrap_err();
        assert!(err.to_string().contains("Unknown flag"), "unknown flags should error");
    }
}

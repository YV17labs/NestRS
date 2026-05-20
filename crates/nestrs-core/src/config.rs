use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::de::DeserializeOwned;

use crate::error::Result;

/// Load configuration from an optional TOML file overlaid with `NESTRS_*` env vars.
pub fn load<T: DeserializeOwned>(toml_path: Option<&str>) -> Result<T> {
    let mut figment = Figment::new();
    if let Some(path) = toml_path {
        figment = figment.merge(Toml::file(path));
    }
    figment = figment.merge(Env::prefixed("NESTRS_").split("__"));
    Ok(figment.extract()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct AppConfig {
        port: u16,
        name: String,
    }

    #[test]
    fn load_from_env_overrides_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_PORT", "4242");
            jail.set_env("NESTRS_NAME", "demo");
            let cfg: AppConfig = load(None).expect("config should load");
            assert_eq!(
                cfg,
                AppConfig {
                    port: 4242,
                    name: "demo".into()
                }
            );
            Ok(())
        });
    }
}

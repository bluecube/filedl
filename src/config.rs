use std::path::PathBuf;

use chrono_tz::{Tz, UTC};
use clap::Parser;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::Deserialize;

fn default_download_url() -> String {
    "/download".into()
}

fn default_timezone() -> Tz {
    UTC
}

fn default_thumbnail_cache_size() -> usize {
    1024 * 1024 * 20
}

#[derive(Debug, Deserialize)]
pub struct Config {
    /// Directory where owned objects are stored
    pub data_path: PathBuf,

    /// Root path for all linked objects
    pub linked_objects_root: PathBuf,

    #[serde(default = "default_download_url")]
    pub download_url: String,

    #[serde(default)]
    pub app_name: String,

    #[serde(default = "default_timezone")]
    pub display_timezone: Tz,

    /// Maximum size in bytes for cached thumbnails.
    #[serde(default = "default_thumbnail_cache_size")]
    pub thumbnail_cache_size: usize,
}

#[derive(Debug, Parser)]
struct Cli {
    /// Location of the config file. If not specified, no config file is loaded.
    #[arg(short, long = "config")]
    config_path: Option<PathBuf>,
}

impl Config {
    pub fn get() -> anyhow::Result<Config> {
        let mut figment = Figment::new();

        let cli = Cli::parse();
        if let Some(config_path) = cli.config_path {
            figment = figment.merge(Toml::file(config_path));
        }
        figment
            .merge(Env::prefixed("FILEDL_"))
            .extract()
            .map_err(anyhow::Error::from)
    }
}

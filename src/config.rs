use std::{
    fs, io,
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    time::Duration,
};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};

use crate::{ping::PingTarget, speedtest::SpeedtestProvider};

pub(crate) fn load_config() -> io::Result<Config> {
    let args = Args::parse();

    if let Some(Command::PrintDefaultConfig) = args.command {
        println!("{}", toml::to_string_pretty(&Config::default()).unwrap());
        std::process::exit(0);
    }

    let config = if let Some(path) = &args.config {
        toml::from_str(&fs::read_to_string(path)?)
            .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?
    } else {
        Config::default()
    };

    Ok(config)
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub(crate) struct Args {
    #[arg(short, long)]
    /// Path to the configuration file
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Prints the default configuration file and exits
    PrintDefaultConfig,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Config {
    pub server: ServerConfig,
    pub ping: PingConfig,
    pub speedtest: SpeedtestConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct ServerConfig {
    pub address: IpAddr,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            address: Ipv4Addr::UNSPECIFIED.into(),
            port: 9090,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct PingConfig {
    pub servers: Vec<PingTarget>,
    #[serde(with = "humantime_serde")]
    pub delay: Duration,
    pub samples: usize,
    pub payload_size: usize,
    pub quantiles: Vec<f64>,
}

impl Default for PingConfig {
    fn default() -> Self {
        Self {
            servers: vec![
                PingTarget::Ip([8, 8, 8, 8].into()),
                PingTarget::Ip([9, 9, 9, 9].into()),
                PingTarget::Ip([1, 1, 1, 1].into()),
                PingTarget::Domain("google.com".to_owned()),
            ],
            delay: Duration::from_secs(1),
            samples: 60,
            payload_size: 512,
            quantiles: vec![0., 0.25, 0.5, 0.75, 0.9, 0.99, 1.],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct SpeedtestConfig {
    pub provider: SpeedtestProvider,
    #[serde(with = "humantime_serde")]
    pub download_duration: Duration,
    #[serde(with = "humantime_serde")]
    pub upload_duration: Duration,
    pub upload_chunk_size: usize,
    pub quantiles: Vec<f64>,
}

impl Default for SpeedtestConfig {
    fn default() -> Self {
        Self {
            provider: SpeedtestProvider::Vodafone,
            download_duration: Duration::from_secs(30),
            upload_duration: Duration::from_secs(30),
            upload_chunk_size: 1_000_000,
            quantiles: vec![0., 0.25, 0.5, 0.75, 0.9, 0.99, 1.],
        }
    }
}

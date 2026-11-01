use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::speedtest::{
    SpeedtestData, SpeedtestMeasurements, SpeedtestProvider, SpeedtestSample,
    SpeedtestSamplePackets,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Iperf3SpeedtestProvider {
    pub target_name: String,
    #[serde(rename = "server")]
    pub servers: Vec<Iperf3Server>,
    #[serde(
        with = "humantime_serde",
        default = "Iperf3SpeedtestProvider::default_retry_delay"
    )]
    pub retry_delay: Duration,
    #[serde(default = "Iperf3SpeedtestProvider::default_retry_amount")]
    pub retry_amount: u32,
    #[serde(
        with = "humantime_serde",
        default = "Iperf3SpeedtestProvider::default_duration"
    )]
    pub download_duration: Duration,
    #[serde(
        with = "humantime_serde",
        default = "Iperf3SpeedtestProvider::default_duration"
    )]
    pub upload_duration: Duration,
    #[serde(
        with = "humantime_serde",
        default = "Iperf3SpeedtestProvider::default_warmup_time"
    )]
    pub warmup_time: Duration,
    #[serde(default)]
    pub bitrate: u32,
    #[serde(default = "Iperf3SpeedtestProvider::default_parallel")]
    pub parallel: NonZeroU32,
    #[serde(default)]
    pub rsa_public_key_path: Option<PathBuf>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub iperf3: Option<PathBuf>,
}

#[async_trait]
impl SpeedtestProvider for Iperf3SpeedtestProvider {
    async fn measure(&self) -> SpeedtestMeasurements {
        let [down, up] = [
            self.run_iperf3_retry(Side::Down).await,
            self.run_iperf3_retry(Side::Up).await,
        ]
        .map(|result| result.map(|ips| self.process_iperf3_summary(ips)));

        SpeedtestMeasurements {
            target: self.target_name.clone(),
            up: up.map_err(crate::format_debug),
            down: down.map_err(crate::format_debug),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Iperf3Server {
    pub host: String,
    #[serde(default = "Iperf3Server::default_port")]
    pub port: u16,
}

impl Iperf3Server {
    fn default_port() -> u16 {
        5201
    }
}

impl Iperf3SpeedtestProvider {
    fn default_retry_delay() -> Duration {
        Duration::from_secs(10)
    }

    fn default_retry_amount() -> u32 {
        6
    }

    fn default_warmup_time() -> Duration {
        Duration::from_secs(1)
    }

    fn default_parallel() -> NonZeroU32 {
        NonZeroU32::MIN
    }

    fn default_duration() -> Duration {
        Duration::from_secs(30)
    }

    fn process_iperf3_summary(&self, summary: Iperf3Summary) -> SpeedtestData {
        let mut samples =
            Vec::with_capacity(self.parallel.get() as usize * summary.intervals.len());

        let iperf_streams = summary
            .intervals
            .into_iter()
            .map(|i| i.streams.into_iter())
            .flatten()
            .filter(|s| !s.omitted);
        for stream in iperf_streams {
            let Ok(sample) = stream.try_into() else {
                core::hint::cold_path();
                continue;
            };
            samples.push(sample);
        }

        SpeedtestData {
            samples,
            total: summary.end.sum.try_into().unwrap_or_default(),
        }
    }

    async fn run_iperf3_retry(&self, side: Side) -> Result<Iperf3Summary, Iperf3Error> {
        for attempt in 1.. {
            match self.run_iperf3(side).await {
                Ok(ips) => return Ok(ips),
                Err(Iperf3Error::NoServersAvailable) if attempt < self.retry_amount => {
                    tokio::time::sleep(self.retry_delay.into()).await;
                    continue;
                }
                Err(other) => return Err(other),
            }
        }
        unreachable!()
    }

    async fn run_iperf3(&self, side: Side) -> Result<Iperf3Summary, Iperf3Error> {
        for server in &self.servers {
            let mut command = self.create_command(side, server);
            let output = tokio::task::spawn_blocking(move || command.output())
                .await
                .expect("iperf3 spawn panic")?;
            if !output.status.success() {
                continue;
            }
            let summary: Iperf3Summary = serde_json::from_slice(&output.stdout)?;
            if let Some(error) = summary.error {
                // This should never be reachable since the status is success
                core::hint::cold_path();
                return Err(Iperf3Error::Iperf3(error));
            }
            return Ok(summary);
        }
        Err(Iperf3Error::NoServersAvailable)
    }

    fn create_command(&self, side: Side, server: &Iperf3Server) -> Command {
        let mut command = Command::new(self.iperf3.as_deref().unwrap_or(Path::new("iperf3")));
        command
            .args([
                "-c",
                &server.host,
                "-p",
                &server.port.to_string(),
                "-i",
                "1",
                "--json",
                "--udp",
                "--time",
                &match side {
                    Side::Down => self.download_duration,
                    Side::Up => self.upload_duration,
                }
                .as_secs()
                .to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if !self.warmup_time.is_zero() {
            command.args(["--omit", &self.warmup_time.as_secs().to_string()]);
        }
        if self.bitrate != 0 {
            command.args(["--bitrate", &self.bitrate.to_string()]);
        }
        if self.parallel.get() != 1 {
            command.args(["--parallel", &self.parallel.to_string()]);
        }
        if side == Side::Down {
            command.arg("--reverse");
        }
        if let Some(path) = &self.rsa_public_key_path {
            command.arg("--rsa-public-key-path").arg(path);
        }
        if let (Some(username), Some(password)) = (&self.username, &self.password) {
            command
                .args(["--username", username])
                .env("IPERF3_PASSWORD", password);
        }
        command
    }
}

#[derive(thiserror::Error, Debug)]
enum Iperf3Error {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("No servers available")]
    NoServersAvailable,
    #[error("Iperf3: {0}")]
    Iperf3(String),
}

#[derive(Deserialize)]
struct Iperf3Summary {
    intervals: Vec<Iperf3SummaryInterval>,
    end: Iperf3SummaryEnd,
    /// Example: the server is busy running a test. try again later
    error: Option<String>,
}

#[derive(Deserialize)]
struct Iperf3SummaryInterval {
    streams: Vec<Iperf3SummaryStream>,
    //sum: Iperf3SummaryStream,
}

#[derive(Deserialize)]
struct Iperf3SummaryEnd {
    #[serde(default)]
    sum: Iperf3SummaryStream,
}

#[derive(Deserialize, Default)]
struct Iperf3SummaryStream {
    bytes: f64,
    seconds: f64,
    packets: u32,
    #[serde(default)]
    lost_packets: Option<u32>,
    #[serde(default)]
    omitted: bool,
}

impl TryFrom<Iperf3SummaryStream> for SpeedtestSample {
    type Error = ();

    fn try_from(stream: Iperf3SummaryStream) -> Result<Self, Self::Error> {
        let Some(packets) = NonZeroU32::new(stream.packets) else {
            core::hint::cold_path();
            return Err(());
        };
        Ok(SpeedtestSample {
            bytes: stream.bytes,
            seconds: stream.seconds,
            packets: stream.lost_packets.map(|lost| SpeedtestSamplePackets {
                count: packets,
                lost,
            }),
        })
    }
}

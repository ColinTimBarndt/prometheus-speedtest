use std::{
    iter::Sum,
    num::NonZeroU32,
    ops::{self, Div},
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{HistogramData, speedtest::iperf3::Iperf3SpeedtestProvider, state::AppState};

use self::http::HttpSpeedtestProvider;

pub mod http;
pub mod iperf3;
mod prometheus;

pub(crate) async fn perform_speedtest(state: Arc<AppState>) -> Vec<SpeedtestResult> {
    let mut measurements = Vec::new();
    for provider in &state.config.speedtest.providers {
        measurements.push(provider.measure().await);
    }
    measurements
        .into_iter()
        .map(|measurements| {
            SpeedtestResult::digest_measurements(
                measurements,
                &state.config.speedtest.speed_quantiles,
                &state.config.speedtest.packet_loss_quantiles,
            )
        })
        .collect()
}

pub struct SpeedtestData {
    pub samples: Vec<SpeedtestSample>,
    pub total: SpeedtestSample,
}

pub struct SpeedtestMeasurements {
    pub target: String,
    pub up: Result<SpeedtestData, String>,
    pub down: Result<SpeedtestData, String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SpeedtestSample {
    pub bytes: f64,
    pub seconds: f64,
    pub packets: Option<SpeedtestSamplePackets>,
}

#[derive(Debug, Clone, Copy)]
pub struct SpeedtestSamplePackets {
    pub count: NonZeroU32,
    pub lost: u32,
}

impl SpeedtestSample {
    /// Bits per second
    pub fn bps(&self) -> f64 {
        self.bytes / self.seconds * 8.
    }

    /// Packet loss ratio or NAN if not available.
    pub fn loss(&self) -> f64 {
        self.packets
            .map(|p| p.lost as f64 / p.count.get() as f64)
            .unwrap_or(f64::NAN)
    }
}

impl ops::Add for SpeedtestSample {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            bytes: self.bytes + rhs.bytes,
            seconds: self.seconds + rhs.seconds,
            packets: match (self.packets, rhs.packets) {
                (Some(l), Some(r)) => Some(l + r),
                _ => None,
            },
        }
    }
}

impl ops::AddAssign for SpeedtestSample {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl ops::Add for SpeedtestSamplePackets {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        match self.count.checked_add(rhs.count.get()) {
            // if the sum would overflow, pick the larger sample
            None => {
                core::hint::cold_path();
                if self.count > rhs.count { self } else { rhs }
            }
            Some(count) => Self {
                count,
                // lost is always <= count and thus cannot overflow here
                // since the sum of counts does not overflow.
                lost: self.lost + rhs.lost,
            },
        }
    }
}

impl Sum for SpeedtestSample {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        let mut sum = SpeedtestSample::default();
        for data in iter {
            sum += data;
        }
        sum
    }
}

#[async_trait]
pub trait SpeedtestProvider: Serialize + Deserialize<'static> + 'static {
    async fn measure(&self) -> SpeedtestMeasurements;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StandardSpeedtestProvider {
    Http(HttpSpeedtestProvider),
    Iperf3(Iperf3SpeedtestProvider),
}

#[async_trait]
impl SpeedtestProvider for StandardSpeedtestProvider {
    async fn measure(&self) -> SpeedtestMeasurements {
        match self {
            Self::Http(p) => p.measure(),
            Self::Iperf3(p) => p.measure(),
        }
        .await
    }
}

#[derive(Debug, Serialize)]
pub struct SpeedtestResult {
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub up_summary: Option<SpeedtestSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub up_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub down_summary: Option<SpeedtestSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub down_error: Option<String>,
}

impl SpeedtestResult {
    pub fn digest_measurements(
        measurements: SpeedtestMeasurements,
        speed_quantiles: &[f64],
        packet_loss_quantiles: &[f64],
    ) -> Self {
        let [(up_summary, up_error), (down_summary, down_error)] =
            [measurements.up, measurements.down].map(|r| match r {
                Ok(ref d) => (
                    Some(SpeedtestSummary::digest_data(
                        d,
                        speed_quantiles,
                        packet_loss_quantiles,
                    )),
                    None,
                ),
                Err(e) => (None, Some(e)),
            });
        Self {
            target: measurements.target,
            up_summary,
            up_error,
            down_summary,
            down_error,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SpeedtestSummary {
    pub speed: HistogramData,
    pub packet_loss: Option<HistogramData>,
}

impl SpeedtestSummary {
    pub fn digest_data(
        SpeedtestData { samples, total }: &SpeedtestData,
        speed_quantiles: &[f64],
        packet_loss_quantiles: &[f64],
    ) -> Self {
        SpeedtestSummary {
            speed: Self::digest_weighted_histogram(
                samples,
                total,
                speed_quantiles,
                |s| s.seconds,
                SpeedtestSample::bps,
            ),
            packet_loss: Self::digest_packet_loss(samples, total, packet_loss_quantiles),
        }
    }

    fn digest_packet_loss(
        samples: &[SpeedtestSample],
        total: &SpeedtestSample,
        quantiles: &[f64],
    ) -> Option<HistogramData> {
        if samples.iter().any(|s| s.packets.is_none()) {
            return None;
        }
        Some(Self::digest_weighted_histogram(
            samples,
            total,
            quantiles,
            |s| s.packets.unwrap().count.get() as f64,
            SpeedtestSample::loss,
        ))
    }

    fn digest_weighted_histogram(
        samples: &[SpeedtestSample],
        total: &SpeedtestSample,
        quantiles: &[f64],
        get_weight: impl Fn(&SpeedtestSample) -> f64,
        get_mean: impl Fn(&SpeedtestSample) -> f64,
    ) -> HistogramData {
        struct MappedSample {
            weight: f64,
            mean: f64,
        }
        let total = MappedSample {
            weight: get_weight(total),
            mean: get_mean(total),
        };
        let mut samples: Vec<_> = samples
            .iter()
            .map(|s| MappedSample {
                weight: get_weight(s),
                mean: get_mean(s),
            })
            .collect();
        samples.sort_unstable_by(|a, b| a.mean.total_cmp(&b.mean));

        let mut quantiles_map = Vec::with_capacity(quantiles.len());
        if !quantiles.is_empty() && total.weight > 0. {
            let mut covered_weight = 0.;
            let mut current_quantile = 0;
            'outer: for sample in samples.iter() {
                // Weighted quantiles with C = 1/2
                let current_weight = covered_weight + sample.weight * 0.5;
                covered_weight += sample.weight;

                loop {
                    let q = quantiles[current_quantile];
                    if current_weight / total.weight >= q {
                        quantiles_map.push((q, sample.mean));
                        current_quantile += 1;
                        if current_quantile >= quantiles.len() {
                            break 'outer;
                        }
                    } else {
                        break;
                    }
                }
            }
            // Remaining quantiles are >= 1.0
            if current_quantile < quantiles.len() {
                let last = samples.last().unwrap().mean.try_into().unwrap();
                for &q in &quantiles[current_quantile..] {
                    if q >= 1. {
                        quantiles_map.push((q, last));
                    }
                }
            }
        }

        let stddev = samples
            .iter()
            .map(|x| {
                let diff = total.mean - x.mean;
                x.weight * (diff * diff)
            })
            .sum::<f64>()
            .div(total.weight)
            .sqrt();

        HistogramData {
            quantiles: quantiles_map,
            mean: total.mean,
            stddev,
            sum: samples
                .iter()
                .map(|s| s.mean)
                .sum::<f64>()
                .try_into()
                .unwrap(),
            count: samples.len(),
        }
    }
}

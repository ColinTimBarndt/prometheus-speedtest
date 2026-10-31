use std::{
    iter::Sum,
    ops::{self, Div},
    sync::Arc,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

use self::http::HttpSpeedtestProvider;

pub mod http;
mod prometheus;

pub(crate) async fn perform_speedtest(state: Arc<AppState>) -> Vec<SpeedtestResult> {
    let mut measurements = Vec::new();
    for provider in &state.config.speedtest.providers {
        measurements.push(provider.measure().await);
    }
    measurements
        .into_iter()
        .map(|measurements| {
            SpeedtestResult::digest_measurements(measurements, &state.config.speedtest.quantiles)
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
}

impl SpeedtestSample {
    /// Bits per second
    pub fn bps(&self) -> f64 {
        self.bytes / self.seconds * 8.
    }
}

impl ops::Add for SpeedtestSample {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            bytes: self.bytes + rhs.bytes,
            seconds: self.seconds + rhs.seconds,
        }
    }
}

impl ops::AddAssign for SpeedtestSample {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
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
}

#[async_trait]
impl SpeedtestProvider for StandardSpeedtestProvider {
    async fn measure(&self) -> SpeedtestMeasurements {
        match self {
            Self::Http(p) => p.measure(),
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
    pub fn digest_measurements(measurements: SpeedtestMeasurements, quantiles: &[f64]) -> Self {
        let [(up_summary, up_error), (down_summary, down_error)] =
            [measurements.up, measurements.down].map(|r| match r {
                Ok(d) => (Some(SpeedtestSummary::digest_data(d, quantiles)), None),
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
    pub quantiles: Vec<(f64, f64)>,
    pub mean: f64,
    pub stddev: f64,
    pub sum: f64,
    pub count: usize,
}

impl SpeedtestSummary {
    pub fn digest_data(
        SpeedtestData { mut samples, total }: SpeedtestData,
        quantiles: &[f64],
    ) -> Self {
        samples.sort_unstable_by(|a, b| a.bps().total_cmp(&b.bps()));

        let mut quantiles_map = Vec::with_capacity(quantiles.len());
        if !quantiles.is_empty() && total.seconds > 0. {
            let mut covered_seconds = 0.;
            let mut current_quantile = 0;
            'outer: for sample in samples.iter() {
                // Weighted quantiles with C = 1/2
                let current_seconds = covered_seconds + sample.seconds * 0.5;
                covered_seconds += sample.seconds;

                loop {
                    let q = quantiles[current_quantile];
                    if current_seconds / total.seconds >= q {
                        quantiles_map.push((q, sample.bps().try_into().unwrap()));
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
                let last = samples.last().unwrap().bps().try_into().unwrap();
                for &q in &quantiles[current_quantile..] {
                    if q >= 1. {
                        quantiles_map.push((q, last));
                    }
                }
            }
        }

        let mean = total.bps();
        let stddev = samples
            .iter()
            .map(|x| {
                let diff = mean - x.bps();
                x.seconds * (diff * diff)
            })
            .sum::<f64>()
            .div(total.seconds)
            .sqrt();

        SpeedtestSummary {
            quantiles: quantiles_map,
            mean: mean.try_into().unwrap(),
            stddev,
            sum: samples
                .iter()
                .map(SpeedtestSample::bps)
                .sum::<f64>()
                .try_into()
                .unwrap(),
            count: samples.len(),
        }
    }
}

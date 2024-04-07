use std::{
    future::Future,
    iter::Sum,
    ops::{self, Div},
    pin::Pin,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::prometheus::{ExpositionBuilder, MetricType, PName};

use self::http::HttpSpeedtestProvider;

pub mod http;

pub struct SpeedtestData {
    pub samples: Vec<SpeedtestSample>,
    pub total: SpeedtestSample,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SpeedtestSample {
    pub bytes: f64,
    pub seconds: f64,
}

impl SpeedtestSample {
    /// Bits per second
    pub fn bps(&self) -> i64 {
        (self.bytes / self.seconds) as i64 * 8
    }

    pub fn bps_f64(&self) -> f64 {
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
    async fn measure_download(&self) -> reqwest::Result<SpeedtestData>;
    async fn measure_upload(&self) -> reqwest::Result<SpeedtestData>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StandardSpeedtestProvider {
    Http(HttpSpeedtestProvider),
}

impl SpeedtestProvider for StandardSpeedtestProvider {
    fn measure_download<'s, 'out>(
        &'s self,
    ) -> Pin<Box<dyn Future<Output = reqwest::Result<SpeedtestData>> + Send + 'out>>
    where
        's: 'out,
        Self: 'out,
    {
        match self {
            Self::Http(p) => p.measure_download(),
        }
    }

    fn measure_upload<'s, 'out>(
        &'s self,
    ) -> Pin<Box<dyn Future<Output = reqwest::Result<SpeedtestData>> + Send + 'out>>
    where
        's: 'out,
        Self: 'out,
    {
        match self {
            Self::Http(p) => p.measure_upload(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SpeedtestSummary {
    pub quantiles: Vec<(f64, u64)>,
    pub mean: u64,
    pub stddev: f64,
    pub sum: u64,
    pub count: usize,
}

impl SpeedtestSummary {
    pub fn digest_data(
        SpeedtestData { mut samples, total }: SpeedtestData,
        quantiles: &[f64],
    ) -> Self {
        samples.sort_unstable_by_key(|d| d.bps());

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
        let meanf = total.bps_f64();
        let stddev = samples
            .iter()
            .map(|x| {
                let diff = meanf - x.bps_f64();
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
                .sum::<i64>()
                .try_into()
                .unwrap(),
            count: samples.len(),
        }
    }

    pub fn write_prometheus(&self, builder: &mut ExpositionBuilder) {
        builder.add_metric(
            PName::new("network_speed_bps").unwrap(),
            MetricType::Summary,
            "network speed in bits per second",
            |mut builder| {
                for (quantile, value) in &self.quantiles {
                    builder.add_line_labeled(
                        PName::QUANTILE,
                        quantile.to_string().as_str(),
                        value,
                        None,
                    );
                }
                builder.with_name(PName::SUFFIX_SUM, |builder| {
                    builder.add_line(&self.sum, None);
                });
                builder.with_name(PName::SUFFIX_COUNT, |builder| {
                    builder.add_line(&self.count, None);
                });
            },
        );

        builder.add_metric(
            PName::new("network_speed_mean_bps").unwrap(),
            MetricType::Gauge,
            "mean network speed in bits per second",
            |mut builder| builder.add_line(&self.mean, None),
        );

        builder.add_metric(
            PName::new("network_speed_stddev").unwrap(),
            MetricType::Gauge,
            "network speed standard deviation",
            |mut builder| builder.add_line(&self.stddev, None),
        );
    }
}

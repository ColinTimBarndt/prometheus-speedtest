use std::sync::Arc;

use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    prometheus::{ExpositionBuilder, MetricType, PName},
};

pub mod vodafone;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SpeedtestProvider {
    Vodafone,
}

impl SpeedtestProvider {
    pub(crate) async fn measure_download(&self, config: Arc<Config>) -> reqwest::Result<Vec<u64>> {
        match self {
            Self::Vodafone => vodafone::measure_download(config).await,
        }
    }

    pub(crate) async fn measure_upload(&self, config: Arc<Config>) -> reqwest::Result<Vec<u64>> {
        match self {
            Self::Vodafone => vodafone::measure_upload(config).await,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SpeedtestSummary {
    pub quantiles: Vec<(f64, u64)>,
    pub mean: f64,
    pub stddev: f64,
    pub count: usize,
}

impl SpeedtestSummary {
    pub fn digest_data(rates: Vec<u64>, quantiles: &[f64]) -> Self {
        let mut hist = Histogram::<u64>::new(0).unwrap();
        for data in &rates {
            hist += *data;
        }

        SpeedtestSummary {
            quantiles: quantiles
                .iter()
                .cloned()
                .map(|q| (q, hist.value_at_quantile(q)))
                .collect(),
            mean: hist.mean(),
            stddev: hist.stdev(),
            count: rates.len(),
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

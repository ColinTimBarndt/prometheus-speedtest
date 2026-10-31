use prometheus::proto::{MetricFamily, MetricType};

use crate::{
    prometheus::{
        FamilyInfo, MetricInfo, SummaryData, create_family, create_gauge, create_summary,
    },
    speedtest::{SpeedtestResult, SpeedtestSummary},
};

impl SpeedtestResult {
    pub fn range_to_prometheus(range: &[Self]) -> Vec<MetricFamily> {
        let mut metrics = PrometheusMetrics::new();
        for result in range {
            metrics.add_result(result);
        }
        metrics.to_vec()
    }
}

struct PrometheusMetrics {
    network_speed_bps: MetricFamily,
    network_speed_mean_bps: MetricFamily,
    network_speed_stddev: MetricFamily,
    speedtest_errors: MetricFamily,
}

impl PrometheusMetrics {
    fn new() -> Self {
        Self {
            network_speed_bps: create_family(
                FamilyInfo::new("network_speed_bps", "network speed in bits per second"),
                MetricType::SUMMARY,
            ),
            network_speed_mean_bps: create_family(
                FamilyInfo::new(
                    "network_speed_mean_bps",
                    "mean network speed in bits per second",
                ),
                MetricType::GAUGE,
            ),
            network_speed_stddev: create_family(
                FamilyInfo::new("network_speed_stddev", "network speed standard deviation"),
                MetricType::GAUGE,
            ),
            speedtest_errors: create_family(
                FamilyInfo::new("ping_errors", "number of ping errors"),
                MetricType::GAUGE,
            ),
        }
    }

    pub fn to_vec(self) -> Vec<MetricFamily> {
        vec![
            self.network_speed_bps,
            self.network_speed_mean_bps,
            self.network_speed_stddev,
            self.speedtest_errors,
        ]
    }

    fn add_result(&mut self, result: &SpeedtestResult) {
        let label = || ("target".to_owned(), result.target.clone());
        for (summary_opt, direction) in [(&result.up_summary, "up"), (&result.down_summary, "down")]
        {
            let Some(summary) = summary_opt else {
                continue;
            };
            self.add_summary(&result.target, direction, summary);
        }
        for error in result.up_error.iter().chain(&result.down_error) {
            self.speedtest_errors.metric.push(create_gauge(
                MetricInfo::new(&mut [("error".to_owned(), error.to_owned()), label()].into_iter()),
                1.,
            ));
        }
    }

    fn add_summary(&mut self, target: &str, direction: &str, summary: &SpeedtestSummary) {
        let labels = move || {
            [
                ("direction".to_owned(), direction.to_owned()),
                ("target".to_owned(), target.to_owned()),
            ]
            .into_iter()
        };
        self.network_speed_bps.metric.push(create_summary(
            MetricInfo::new(&mut labels()),
            SummaryData {
                count: summary.count as u64,
                sum: summary.sum,
                quantiles: &mut summary.quantiles.iter().copied(),
            },
        ));

        self.network_speed_mean_bps
            .metric
            .push(create_gauge(MetricInfo::new(&mut labels()), summary.mean));

        self.network_speed_stddev
            .metric
            .push(create_gauge(MetricInfo::new(&mut labels()), summary.stddev));
    }
}

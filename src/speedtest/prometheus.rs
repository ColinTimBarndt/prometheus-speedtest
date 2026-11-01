use prometheus::proto::{MetricFamily, MetricType};

use crate::{
    prometheus::{FamilyInfo, MetricInfo, RangeToPrometheus, create_family, create_gauge},
    speedtest::{SpeedtestResult, SpeedtestSummary},
};

impl RangeToPrometheus for SpeedtestResult {
    fn range_to_prometheus(range: &[Self]) -> Vec<MetricFamily> {
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
    network_speed_stddev_bps: MetricFamily,
    packet_loss: MetricFamily,
    packet_loss_mean: MetricFamily,
    packet_loss_stddev: MetricFamily,
    errors: MetricFamily,
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
            network_speed_stddev_bps: create_family(
                FamilyInfo::new(
                    "network_speed_stddev_bps",
                    "network speed standard deviation",
                ),
                MetricType::GAUGE,
            ),
            packet_loss: create_family(
                FamilyInfo::new("packet_loss", "packet loss ratio"),
                MetricType::SUMMARY,
            ),
            packet_loss_mean: create_family(
                FamilyInfo::new("packet_loss_mean", "mean packet loss"),
                MetricType::GAUGE,
            ),
            packet_loss_stddev: create_family(
                FamilyInfo::new("packet_loss_stddev", "packet loss standard deviation"),
                MetricType::GAUGE,
            ),
            errors: create_family(
                FamilyInfo::new("errors", "number of speedtest errors"),
                MetricType::GAUGE,
            ),
        }
    }

    pub fn to_vec(self) -> Vec<MetricFamily> {
        let mut results = vec![
            self.network_speed_bps,
            self.network_speed_mean_bps,
            self.network_speed_stddev_bps,
            self.packet_loss,
            self.packet_loss_mean,
            self.packet_loss_stddev,
            self.errors,
        ];
        results.retain(|f| !f.metric.is_empty());
        results
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
            self.errors.metric.push(create_gauge(
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
        summary.speed.add_to_prometheus(
            labels,
            &mut self.network_speed_bps,
            &mut self.network_speed_mean_bps,
            &mut self.network_speed_stddev_bps,
        );
        if let Some(loss) = &summary.packet_loss {
            loss.add_to_prometheus(
                labels,
                &mut self.packet_loss,
                &mut self.packet_loss_mean,
                &mut self.packet_loss_stddev,
            );
        }
    }
}

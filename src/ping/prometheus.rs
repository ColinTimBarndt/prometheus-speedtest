use prometheus::proto::{MetricFamily, MetricType};

use crate::{
    ping::{PingResult, PingSummary},
    prometheus::{FamilyInfo, MetricInfo, RangeToPrometheus, create_family, create_gauge},
};

impl RangeToPrometheus for PingResult {
    fn range_to_prometheus(range: &[Self]) -> Vec<MetricFamily> {
        let mut metrics = PrometheusMetrics::new();
        for result in range {
            metrics.add_result(result);
        }
        metrics.to_vec()
    }
}

struct PrometheusMetrics {
    ping_ms: MetricFamily,
    ping_mean_ms: MetricFamily,
    ping_stddev_ms: MetricFamily,
    packet_loss: MetricFamily,
    ping_errors: MetricFamily,
}

impl PrometheusMetrics {
    fn new() -> Self {
        Self {
            ping_ms: create_family(
                FamilyInfo::new("ping_ms", "ping to target"),
                MetricType::SUMMARY,
            ),
            ping_mean_ms: create_family(
                FamilyInfo::new("ping_mean_ms", "mean latency"),
                MetricType::GAUGE,
            ),
            ping_stddev_ms: create_family(
                FamilyInfo::new("ping_stddev_ms", "latency standard deviation"),
                MetricType::GAUGE,
            ),
            packet_loss: create_family(
                FamilyInfo::new("packet_loss", "packet loss ratio"),
                MetricType::GAUGE,
            ),
            ping_errors: create_family(
                FamilyInfo::new("ping_errors", "number of ping errors"),
                MetricType::GAUGE,
            ),
        }
    }

    pub fn to_vec(self) -> Vec<MetricFamily> {
        let mut results = vec![
            self.ping_ms,
            self.ping_mean_ms,
            self.ping_stddev_ms,
            self.packet_loss,
            self.ping_errors,
        ];
        results.retain(|f| !f.metric.is_empty());
        results
    }

    fn add_result(&mut self, result: &PingResult) {
        let target = result.target.to_string();
        let label = move || ("target".to_owned(), result.target.to_string());
        if let Some(summary) = &result.summary {
            self.add_summary(&target, &summary);
        }
        if let Some(error) = &result.error {
            self.ping_errors.metric.push(create_gauge(
                MetricInfo::new(&mut [("error".to_owned(), error.to_owned()), label()].into_iter()),
                1.,
            ));
        }
    }

    fn add_summary(&mut self, target: &str, summary: &PingSummary) {
        let labels = move || core::iter::once(("target".to_owned(), target.to_owned()));
        summary.ping.add_to_prometheus(
            labels,
            &mut self.ping_ms,
            &mut self.ping_mean_ms,
            &mut self.ping_stddev_ms,
        );

        self.ping_errors
            .metric
            .extend(summary.errors.iter().map(|(kind, &count)| {
                create_gauge(
                    MetricInfo::new(
                        &mut core::iter::once(("error".to_owned(), kind.to_string()))
                            .chain(labels()),
                    ),
                    count.into(),
                )
            }));
    }
}

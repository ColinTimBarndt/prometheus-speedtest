use prometheus::proto::{MetricFamily, MetricType};

use crate::{
    ping::{PingResult, PingSummary},
    prometheus::{
        FamilyInfo, MetricInfo, SummaryData, create_family, create_gauge, create_summary,
    },
};

impl PingResult {
    pub fn range_to_prometheus(range: &[Self]) -> Vec<MetricFamily> {
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
    ping_stddev: MetricFamily,
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
            ping_stddev: create_family(
                FamilyInfo::new("ping_stddev", "latency standard deviation"),
                MetricType::GAUGE,
            ),
            packet_loss: create_family(
                FamilyInfo::new("packet_loss", "packet loss % (0 to 1)"),
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
            self.ping_stddev,
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
        let label = move || ("target".to_owned(), target.to_owned());
        self.ping_ms.metric.push(create_summary(
            MetricInfo::new(&mut std::iter::once(label())),
            SummaryData {
                count: summary.count as u64,
                sum: summary.sum,
                quantiles: &mut summary.quantiles.iter().copied(),
            },
        ));

        self.ping_mean_ms.metric.push(create_gauge(
            MetricInfo::new(&mut std::iter::once(label())),
            summary.mean_ms,
        ));

        self.ping_stddev.metric.push(create_gauge(
            MetricInfo::new(&mut std::iter::once(label())),
            summary.stddev,
        ));

        self.packet_loss.metric.push(create_gauge(
            MetricInfo::new(&mut std::iter::once(label())),
            summary.loss_percent,
        ));

        self.ping_errors
            .metric
            .extend(summary.errors.iter().map(|(kind, &count)| {
                create_gauge(
                    MetricInfo::new(
                        &mut [("error".to_owned(), kind.to_string()), label()].into_iter(),
                    ),
                    count.into(),
                )
            }));
    }
}

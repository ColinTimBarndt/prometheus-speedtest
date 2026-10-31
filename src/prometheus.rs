use std::time::SystemTime;

use prometheus::proto as p;

pub struct FamilyInfo<'a> {
    pub name: &'a str,
    pub help: &'a str,
}

impl<'a> FamilyInfo<'a> {
    pub fn new(name: &'a str, help: &'a str) -> Self {
        Self { name, help }
    }
}

pub struct MetricInfo<'a> {
    pub labels: &'a mut dyn Iterator<Item = (String, String)>,
    pub timestamp: Option<SystemTime>,
}

impl<'a> MetricInfo<'a> {
    pub fn new(labels: &'a mut impl Iterator<Item = (String, String)>) -> Self {
        Self {
            labels: labels,
            timestamp: None,
        }
    }

    pub fn at(mut self, time: SystemTime) -> Self {
        self.timestamp = Some(time);
        self
    }
}

pub fn create_family(info: FamilyInfo, ty: p::MetricType) -> p::MetricFamily {
    let mut family = p::MetricFamily::new();
    family.set_name(info.name.to_owned());
    family.set_help(info.help.to_owned());
    family.set_type(ty);
    family
}

pub fn create_metric(info: MetricInfo) -> p::Metric {
    let mut metric = p::Metric::new();
    for (key, value) in info.labels {
        let mut pair = p::LabelPair::new();
        pair.set_name(key);
        pair.set_value(value);
        metric.label.push(pair);
    }
    if let Some(ms) = info
        .timestamp
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| d.as_millis().try_into().ok())
    {
        metric.set_timestamp_ms(ms);
    }
    metric
}

pub fn create_gauge(info: MetricInfo, value: f64) -> p::Metric {
    let mut metric = create_metric(info);
    let mut gauge = p::Gauge::new();
    gauge.set_value(value);
    metric.set_gauge(gauge);
    metric
}

pub fn create_gauge_family(f_info: FamilyInfo, m_info: MetricInfo, value: f64) -> p::MetricFamily {
    let mut family = create_family(f_info, p::MetricType::GAUGE);
    family.mut_metric().push(create_gauge(m_info, value));
    family
}

pub struct SummaryData<'a> {
    pub count: u64,
    pub sum: f64,
    pub quantiles: &'a mut dyn Iterator<Item = (f64, f64)>,
}

pub fn create_summary(info: MetricInfo, data: SummaryData) -> p::Metric {
    let mut metric = create_metric(info);
    let mut summary = p::Summary::new();
    summary.set_sample_count(data.count);
    summary.set_sample_sum(data.sum);
    for (q, v) in data.quantiles {
        let mut quantile = p::Quantile::new();
        quantile.set_quantile(q);
        quantile.set_value(v);
        summary.quantile.push(quantile);
    }
    metric.set_summary(summary);
    metric
}

pub fn create_summary_family(
    f_info: FamilyInfo,
    m_info: MetricInfo,
    data: SummaryData,
) -> p::MetricFamily {
    let mut family = create_family(f_info, p::MetricType::SUMMARY);
    family.mut_metric().push(create_summary(m_info, data));
    family
}

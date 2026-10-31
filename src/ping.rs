mod prometheus;

use core::fmt;
use std::{
    collections::HashMap, fmt::Display, io, net::IpAddr, ops::Div, sync::Arc, time::Duration,
};

use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize, Serializer};
use surge_ping::{IcmpPacket, PingIdentifier, PingSequence, SurgeError};
use thiserror::Error;
use tokio::task::JoinSet;

use crate::{Resolver, state::AppState};

pub(crate) async fn perform_ping(state: Arc<AppState>) -> Vec<PingResult> {
    let mut payload = vec![0; state.config.ping.payload_size].into_boxed_slice();
    rand::fill(&mut payload[..]);
    let payload = Arc::new(payload);

    let mut set = JoinSet::<PingResult>::new();
    for target in state.config.ping.servers.iter().cloned() {
        let payload = payload.clone();
        let state = state.clone();
        set.spawn(async move {
            let addr = match target.resolve(&state.resolver).await {
                Ok(addr) => addr,
                Err(err) => {
                    return PingResult {
                        target,
                        summary: None,
                        error: Some(err.to_string()),
                    };
                }
            };
            let (samples, errors) = sample_pings(
                addr,
                state.config.ping.samples,
                state.config.ping.delay,
                payload,
            )
            .await;
            PingResult {
                target,
                summary: Some(PingSummary::digest_data(
                    samples,
                    errors,
                    &state.config.ping.quantiles,
                )),
                error: None,
            }
        });
    }

    let mut results = Vec::with_capacity(state.config.ping.servers.len());
    while let Some(join_result) = set.join_next().await {
        let result = join_result.unwrap();
        results.push(result);
    }
    results
}

async fn sample_pings(
    addr: IpAddr,
    samples: usize,
    delay: Duration,
    payload: Arc<Box<[u8]>>,
) -> (Vec<f64>, Vec<PingErrorKind>) {
    if samples == 0 {
        return (Vec::new(), Vec::new());
    }
    let client = surge_ping::Client::new(&surge_ping::ConfigBuilder::default().build()).unwrap();

    let mut set = JoinSet::<(usize, Result<(IcmpPacket, Duration), SurgeError>)>::new();
    for seq in 0.. {
        let mut pinger = client.pinger(addr, PingIdentifier(0)).await;
        let payload = payload.clone();
        set.spawn(async move {
            (
                seq,
                pinger.ping(PingSequence(seq as u16), &payload[..]).await,
            )
        });

        if seq < samples {
            tokio::time::sleep(delay).await;
        } else {
            break;
        }
    }
    let mut results = vec![f64::NAN; samples];
    let mut errors = Vec::with_capacity(samples);

    while let Some(join_result) = set.join_next().await {
        let result = join_result.unwrap();
        match result {
            (seq, Ok((_packet, duration))) => {
                results[seq] = duration.as_secs_f64() * 1000.;
            }
            (_, Err(err)) => {
                errors.push(err.into());
            }
        };
    }

    (results, errors)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
pub enum PingTarget {
    Ip(IpAddr),
    Domain(String),
}

#[derive(Debug, Error)]
pub enum PingPrepareError {
    #[error("{0}")]
    ResolveError(#[from] hickory_resolver::net::NetError),
    #[error("no IP address found")]
    NoIp,
}

impl PingTarget {
    pub async fn resolve(&self, resolver: &Resolver) -> Result<IpAddr, PingPrepareError> {
        match self {
            Self::Ip(ip) => Ok(*ip),
            Self::Domain(domain) => resolver
                .lookup_ip(domain)
                .await?
                .into_iter()
                .next()
                .ok_or(PingPrepareError::NoIp),
        }
    }
}

impl Display for PingTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(ip) => <IpAddr as Display>::fmt(ip, f),
            Self::Domain(domain) => f.write_str(domain),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PingResult {
    target: PingTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<PingSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum PingErrorKind {
    #[error("buffer size was too small")]
    IncorrectBufferSize,
    #[error("malformed packet")]
    MalformedPacket,
    #[error("io error: {kind}")]
    IOError {
        #[serde(serialize_with = "serialize_error_kind")]
        kind: io::ErrorKind,
    },
    #[error("timeout")]
    Timeout {
        //#[serde(serialize_with = "serialize_ping_sequence")]
        //seq: PingSequence,
    },
    #[error("Echo Request packet.")]
    EchoRequestPacket,
    #[error("Network error.")]
    NetworkError,
    #[error("Multiple identical request")]
    IdenticalRequests,
}

impl From<SurgeError> for PingErrorKind {
    fn from(value: SurgeError) -> Self {
        match value {
            SurgeError::IncorrectBufferSize => Self::IncorrectBufferSize,
            SurgeError::MalformedPacket(..) => Self::MalformedPacket,
            SurgeError::IOError(err) => Self::IOError { kind: err.kind() },
            SurgeError::Timeout { .. } => Self::Timeout { /*seq*/ },
            SurgeError::EchoRequestPacket => Self::EchoRequestPacket,
            SurgeError::NetworkError => Self::NetworkError,
            SurgeError::IdenticalRequests { .. } => Self::IdenticalRequests,
            SurgeError::ClientDestroyed => todo!(),
        }
    }
}

fn serialize_error_kind<S: Serializer>(
    kind: &io::ErrorKind,
    ser: S,
) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error> {
    ser.serialize_str(&kind.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct PingSummary {
    pub quantiles: Vec<(f64, f64)>,
    pub mean_ms: f64,
    pub stddev: f64,
    pub sum: f64,
    pub count: usize,
    pub loss_percent: f64,
    #[serde(serialize_with = "serialize_error_kind_map")]
    pub errors: HashMap<PingErrorKind, u32>,
}

fn serialize_error_kind_map<S: Serializer>(
    map: &HashMap<PingErrorKind, u32>,
    ser: S,
) -> Result<<S as Serializer>::Ok, <S as Serializer>::Error> {
    use serde::ser::SerializeMap as _;

    let map = map.iter();
    let mut map_ser = ser.serialize_map(Some(map.size_hint().0))?;
    for (k, v) in map {
        map_ser.serialize_entry(&k.to_string(), v)?;
    }
    map_ser.end()
}

impl PingSummary {
    pub fn digest_data(
        mut samples: Vec<f64>,
        errors: Vec<PingErrorKind>,
        quantiles: &[f64],
    ) -> Self {
        let mut error_buckets = HashMap::with_capacity(8);
        for err in errors {
            if let Some(count) = error_buckets.get_mut(&err) {
                *count += 1;
            } else {
                error_buckets.insert(err, 1);
            }
        }

        let mut lost_packets = 0;
        let total_packets = samples.len();
        {
            let mut i = 0;
            while i < samples.len() {
                if samples[i].is_nan() {
                    samples.swap_remove(i);
                    lost_packets += 1;
                } else {
                    i += 1;
                }
            }
        }

        if samples.is_empty() {
            return Self {
                errors: error_buckets,
                quantiles: Vec::new(),
                mean_ms: f64::NAN,
                stddev: f64::NAN,
                sum: f64::NAN,
                count: 0,
                loss_percent: 1.,
            };
        }

        let mut hist = Histogram::<u16>::new(0).unwrap();
        for sample in &samples {
            hist += (*sample * 16.).round() as u64;
        }
        samples.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let n = samples.len();
        let sum = samples.iter().sum();
        let mean_ms = sum / (n as f64);
        Self {
            quantiles: quantiles
                .iter()
                .cloned()
                .map(|q| (q, hist.value_at_quantile(q) as f64 / 16.))
                .collect(),
            mean_ms,
            stddev: samples
                .iter()
                .map(|val| (*val - mean_ms).powi(2))
                .sum::<f64>()
                .div(n as f64)
                .sqrt(),
            sum,
            count: n,
            loss_percent: lost_packets as f64 / total_packets as f64,
            errors: error_buckets,
        }
    }
}

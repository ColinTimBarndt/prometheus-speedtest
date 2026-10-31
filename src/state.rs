use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::{config::Config, ping::PingResult, speedtest::SpeedtestResult};

#[derive(Debug)]
pub struct AppState {
    pub config: Config,
    pub ping: Mutex<EndpointState<PingResult>>,
    pub speedtest: Mutex<EndpointState<SpeedtestResult>>,
    pub resolver: crate::Resolver,
}

impl AppState {
    pub fn new(config: Config, resolver: crate::Resolver) -> Self {
        Self {
            config,
            ping: Default::default(),
            speedtest: Default::default(),
            resolver,
        }
    }
}

#[derive(Debug)]
pub struct EndpointState<T> {
    pub last_measurement: Option<Instant>,
    pub metrics: Vec<T>,
}

impl<T> Default for EndpointState<T> {
    fn default() -> Self {
        Self {
            last_measurement: None,
            metrics: Vec::new(),
        }
    }
}

impl<T> EndpointState<T> {
    pub fn should_use_cache(&self, cache_duration: Duration) -> bool {
        self.last_measurement
            .map(|last| last.elapsed() <= cache_duration)
            .unwrap_or_default()
    }
}

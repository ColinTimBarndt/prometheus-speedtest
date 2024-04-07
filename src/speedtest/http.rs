use core::task;
use std::{
    convert::Infallible,
    pin::Pin,
    time::{Duration, Instant},
};

use axum::{async_trait, body::Bytes};
use http::header;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio_stream::Stream;
use url::Url;

use super::{SpeedtestData as Data, SpeedtestProvider, SpeedtestSample as Sample};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpSpeedtestProvider {
    pub download_endpoint: Url,
    pub upload_endpoint: Url,
    #[serde(with = "humantime_serde")]
    pub download_duration: Duration,
    #[serde(with = "humantime_serde")]
    pub upload_duration: Duration,
    pub upload_chunk_size: usize,
}

#[async_trait]
impl SpeedtestProvider for HttpSpeedtestProvider {
    async fn measure_download(&self) -> reqwest::Result<Data> {
        let mut locals = self.prepare_measurements(self.download_duration);
        self.collect_download_data(&mut locals).await?;
        Ok(self.finish_measurements(locals))
    }

    async fn measure_upload(&self) -> reqwest::Result<Data> {
        let mut locals = self.prepare_measurements(self.download_duration);
        self.collect_upload_data(&mut locals).await?;
        Ok(self.finish_measurements(locals))
    }
}

struct MeasurementLocals {
    client: reqwest::Client,
    start_time: Instant,
    end_time: Instant,
    samples: Vec<Sample>,
    total_bytes: f64,
    last_chunk_time: Instant,
}

impl HttpSpeedtestProvider {
    #[inline(always)]
    fn prepare_measurements(&self, duration: Duration) -> MeasurementLocals {
        let start_time = Instant::now();
        let last_chunk_time = start_time;
        let end_time = start_time + duration;

        MeasurementLocals {
            client: self.build_client(),
            start_time,
            end_time,
            samples: Vec::new(),
            total_bytes: 0.,
            last_chunk_time,
        }
    }

    #[inline(always)]
    fn finish_measurements(&self, locals: MeasurementLocals) -> Data {
        Data {
            samples: locals.samples,
            total: Sample {
                bytes: locals.total_bytes,
                seconds: locals
                    .last_chunk_time
                    .duration_since(locals.start_time)
                    .as_secs_f64(),
            },
        }
    }

    #[inline(always)]
    async fn collect_download_data(&self, locals: &mut MeasurementLocals) -> reqwest::Result<()> {
        // Averages out spikes
        const MIN_SAMPLE_TIME: Duration = Duration::from_millis(50);
        let mut sample_bytes = 0.;

        'outer: loop {
            let mut response = locals
                .client
                .get(self.download_endpoint.clone())
                .send()
                .await?
                .error_for_status()?;

            loop {
                match tokio::time::timeout_at(locals.end_time.into(), response.chunk()).await {
                    Ok(result) => {
                        let Some(chunk) = result? else {
                            break;
                        };
                        let bytes = chunk.len() as f64;
                        locals.total_bytes += bytes;
                        sample_bytes += bytes;
                        let now = Instant::now();
                        if now.duration_since(locals.last_chunk_time) >= MIN_SAMPLE_TIME {
                            locals.samples.push(Sample {
                                bytes: sample_bytes,
                                seconds: now.duration_since(locals.last_chunk_time).as_secs_f64(),
                            });
                            sample_bytes = 0.;
                            locals.last_chunk_time = now;
                        }
                    }
                    Err(_) => break 'outer,
                }
            }
        }
        Ok(())
    }

    #[inline(always)]
    async fn collect_upload_data(&self, locals: &mut MeasurementLocals) -> reqwest::Result<()> {
        let mut data = vec![0; 256];
        rand::thread_rng().fill_bytes(&mut data);
        let data = data; // immutable

        while let Ok(result) = tokio::time::timeout_at(
            locals.end_time.into(),
            self.create_upload(&locals.client, &data),
        )
        .await
        {
            result?;
            let now = Instant::now();
            let size = self.upload_chunk_size as f64;
            locals.samples.push(Sample {
                bytes: size,
                seconds: now.duration_since(locals.last_chunk_time).as_secs_f64(),
            });
            locals.total_bytes += size;
            locals.last_chunk_time = now;
        }
        Ok(())
    }

    async fn create_upload(
        &self,
        client: &reqwest::Client,
        data: &[u8],
    ) -> reqwest::Result<reqwest::Response> {
        client
            .post(self.upload_endpoint.clone())
            .header(
                header::CONTENT_TYPE,
                mime::APPLICATION_OCTET_STREAM.as_ref(),
            )
            .body(reqwest::Body::wrap_stream(Infinistream::new(
                data,
                self.upload_chunk_size,
            )))
            .send()
            .await?
            .error_for_status()
    }

    fn build_client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .no_brotli()
            .no_deflate()
            .no_gzip()
            .build()
            .unwrap()
    }
}

struct Infinistream {
    data: Bytes,
    len: usize,
}

impl Infinistream {
    pub fn new(data: &[u8], len: usize) -> Self {
        assert!(!data.is_empty());
        Self {
            data: Bytes::copy_from_slice(data),
            len,
        }
    }
}

impl Stream for Infinistream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        _cx: &mut task::Context<'_>,
    ) -> task::Poll<Option<Self::Item>> {
        if self.len == 0 {
            return task::Poll::Ready(None);
        }
        let len = self.data.len().min(self.len);
        self.len -= len;
        task::Poll::Ready(Some(Ok(self.data.slice(..len))))
    }
}

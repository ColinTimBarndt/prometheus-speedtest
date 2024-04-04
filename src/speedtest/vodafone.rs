use core::task;
use std::{convert::Infallible, pin::Pin, sync::Arc, time::Instant};

use axum::body::Bytes;
use http::header;
use rand::RngCore;
use tokio_stream::Stream;

use crate::config::Config;

const DOWNLOAD_ENDPOINT: &str = "https://speedtest-64.speedtest.vodafone-ip.de/data.zero.bin.512M";
const UPLOAD_ENDPOINT: &str = "https://speedtest-64.speedtest.vodafone-ip.de/empty.txt";

pub(crate) async fn measure_download(config: Arc<Config>) -> reqwest::Result<Vec<u64>> {
    let mut samples = Vec::new();

    let mut last_time = Instant::now();
    let end_time = last_time + config.speedtest.download_duration;
    let mut response = create_download().await?;
    while let Ok(result) = tokio::time::timeout_at(end_time.into(), response.chunk()).await {
        let now = Instant::now();
        let Some(bytes) = result? else {
            response = create_download().await?;
            last_time = now;
            continue;
        };
        let time = now.duration_since(last_time);
        let size = bytes.len() as u64;
        samples.push(((size * 8) as f32 / time.as_secs_f32()) as u64);
        last_time = now;
    }

    Ok(samples)
}

async fn create_download() -> reqwest::Result<reqwest::Response> {
    reqwest::get(DOWNLOAD_ENDPOINT).await?.error_for_status()
}

pub(crate) async fn measure_upload(config: Arc<Config>) -> reqwest::Result<Vec<u64>> {
    let mut data = vec![0; 256];
    rand::thread_rng().fill_bytes(&mut data);

    let mut samples = Vec::new();

    let client = reqwest::ClientBuilder::new()
        .no_brotli()
        .no_deflate()
        .no_gzip()
        .build()
        .unwrap();

    let size = config.speedtest.upload_chunk_size;

    let mut last_time = Instant::now();
    let end_time = last_time + config.speedtest.upload_duration;
    while let Ok(result) =
        tokio::time::timeout_at(end_time.into(), create_upload(&client, &data, size)).await
    {
        result?;
        let now = Instant::now();
        let time = now.duration_since(last_time);
        samples.push(((size * 8) as f32 / time.as_secs_f32()) as u64);
        last_time = now;
    }

    Ok(samples)
}

async fn create_upload(
    client: &reqwest::Client,
    data: &[u8],
    len: usize,
) -> reqwest::Result<reqwest::Response> {
    client
        .post(UPLOAD_ENDPOINT)
        .header(
            header::CONTENT_TYPE,
            mime::APPLICATION_OCTET_STREAM.as_ref(),
        )
        .body(reqwest::Body::wrap_stream(Infinistream::new(data, len)))
        .send()
        .await?
        .error_for_status()
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

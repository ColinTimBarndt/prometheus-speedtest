use std::{net::SocketAddr, process::ExitCode, sync::Arc, time::Instant};

use ::prometheus::Encoder;
use axum::{
    Router,
    body::Body,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use config::load_config;
use http::{HeaderMap, StatusCode, header};
use lazy_static::lazy_static;
use mime::{
    APPLICATION, APPLICATION_JSON, HTML, JSON, Mime, Name, PLAIN, TEXT, TEXT_HTML, TEXT_HTML_UTF_8,
    TEXT_PLAIN, UTF_8,
};
use tokio::net::TcpListener;

use crate::{
    ping::perform_ping, prometheus::RangeToPrometheus, speedtest::perform_speedtest,
    state::AppState,
};

pub mod config;
pub mod ping;
pub mod prometheus;
pub mod speedtest;
mod state;
mod humanbits;

lazy_static! {
    static ref TEXT_PLAIN_UTF_8_VERSION_4: Mime =
        "text/plain; version=0.0.4; charset=utf-8".parse().unwrap();
    static ref APPLICATION_PROTOBUF_PROMETHEUS: Mime =
        "application/vnd.google.protobuf; proto=io.prometheus.client.MetricFamily; encoding=delimited".parse().unwrap();
    static ref APPLICATION_PROTOBUF: Mime =
        "application/vnd.google.protobuf".parse().unwrap();
    static ref PROTOBUF: Name<'static> = APPLICATION_PROTOBUF_PROMETHEUS.subtype();
}

pub type Resolver = hickory_resolver::TokioResolver;

#[tokio::main]
async fn main() -> ExitCode {
    eprintln!("{}", include_str!("startup-notice.txt"));

    tracing_subscriber::fmt::init();

    let config = match load_config() {
        Ok(c) => c,
        Err(error) => {
            tracing::error!("Could not load config: {error:?}");
            return ExitCode::FAILURE;
        }
    };

    let resolver = match Resolver::builder_tokio().and_then(|b| b.build()) {
        Ok(r) => r,
        Err(error) => {
            tracing::error!("Failed to initialize DNS: {error:?}");
            return ExitCode::FAILURE;
        }
    };

    let bind_to = SocketAddr::new(config.server.address, config.server.port);

    let app = create_router(Arc::new(AppState::new(config, resolver)));

    let listener = match TcpListener::bind(bind_to).await {
        Ok(l) => l,
        Err(error) => {
            tracing::error!("Failed binding to {bind_to}: {error:?}");
            return ExitCode::FAILURE;
        }
    };
    match axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error:?}");
            ExitCode::FAILURE
        }
    }
}

fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(get_index))
        .route("/ping", get(get_ping))
        .route("/speedtest", get(get_speedtest))
        .with_state(state)
}

async fn get_index(headers: HeaderMap) -> impl IntoResponse {
    let is_terminal = {
        headers.get(header::USER_AGENT).is_some_and(|ua| {
            let bytes = ua.as_bytes();
            bytes.starts_with(b"curl/") || bytes.starts_with(b"Wget/")
        })
    };

    let response_type = if let Some(accept) = headers
        .get(header::ACCEPT)
        .and_then(|it| it.to_str().ok())
        .and_then(|it| it.parse::<accept_header::Accept>().ok())
    {
        // Terminals should receive plaintext as default
        let available = if is_terminal {
            [TEXT_PLAIN, TEXT_HTML]
        } else {
            [TEXT_HTML, TEXT_PLAIN]
        };

        'negotiate: {
            for media_type in &accept.types {
                if available.contains(&media_type.mime) {
                    break 'negotiate media_type.mime.clone();
                }
            }

            if accept.wildcard.is_some() {
                break 'negotiate available[0].clone();
            }

            return StatusCode::NOT_ACCEPTABLE.into_response();
        }
    } else {
        TEXT_HTML
    };

    match (response_type.type_(), response_type.subtype()) {
        (TEXT, PLAIN) if is_terminal => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime::TEXT_PLAIN_UTF_8.as_ref())],
            include_str!("public/index.ansi.txt"),
        ),

        (TEXT, PLAIN) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime::TEXT_PLAIN_UTF_8.as_ref())],
            include_str!("public/index.txt"),
        ),

        (TEXT, HTML) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime::TEXT_HTML_UTF_8.as_ref())],
            include_str!("public/index.html"),
        ),

        _ => unreachable!(),
    }
    .into_response()
}

fn negotiate_prometheus_mime(headers: &HeaderMap) -> Result<Mime, StatusCode> {
    let mut response_type = if let Some(accept) = headers
        .get(header::ACCEPT)
        .and_then(|it| it.to_str().ok())
        .and_then(|it| it.parse::<accept_header::Accept>().ok())
    {
        accept
            .negotiate(&[TEXT_PLAIN, APPLICATION_JSON, APPLICATION_PROTOBUF.clone()])
            .map_err(|code| StatusCode::from_u16(code.as_u16()).unwrap())?
    } else {
        TEXT_PLAIN_UTF_8_VERSION_4.clone()
    };
    if response_type.type_() == TEXT {
        match response_type.get_param("version") {
            Some(UTF_8) => (),
            _ => TEXT_PLAIN_UTF_8_VERSION_4.clone_into(&mut response_type),
        }
    } else if response_type == TEXT_HTML {
        response_type = TEXT_HTML_UTF_8;
    } else if &response_type == &*APPLICATION_PROTOBUF {
        APPLICATION_PROTOBUF_PROMETHEUS.clone_into(&mut response_type)
    }
    Ok(response_type)
}

fn encode_prometheus_response<T>(response_type: &Mime, data: &[T]) -> Response<Body>
where
    T: RangeToPrometheus + serde::Serialize,
{
    let response: Vec<u8> = match (response_type.type_(), response_type.subtype()) {
        (TEXT, PLAIN) => ::prometheus::TextEncoder::new()
            .encode_to_string(&T::range_to_prometheus(data))
            .expect("Prometheus text encoding")
            .into_bytes(),
        (APPLICATION, ref sub) if sub == &*PROTOBUF => {
            let mut buf = Vec::new();
            ::prometheus::ProtobufEncoder::new()
                .encode(&T::range_to_prometheus(data), &mut buf)
                .expect("Prometheus Protobuf encoding");
            buf
        }
        (APPLICATION, JSON) => serde_json::to_string_pretty(data)
            .expect("JSON encoding")
            .into_bytes(),
        _ => unreachable!(),
    };

    Response::builder()
        .header(header::CONTENT_TYPE, response_type.as_ref())
        .status(StatusCode::OK)
        .body(Body::from(response))
        .unwrap()
}

async fn get_ping(State(state): State<Arc<AppState>>, headers: HeaderMap) -> impl IntoResponse {
    let response_type = match negotiate_prometheus_mime(&headers) {
        Ok(ty) => ty,
        Err(code) => return code.into_response(),
    };

    let state2 = state.clone();
    let mut ping_state = state2.ping.lock().await;
    if !ping_state.should_use_cache(state.config.ping.cooldown) {
        ping_state.last_measurement = Some(Instant::now());
        ping_state.metrics = perform_ping(state).await;
    };
    let data = &ping_state.metrics[..];

    encode_prometheus_response(&response_type, data)
}

async fn get_speedtest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let response_type = match negotiate_prometheus_mime(&headers) {
        Ok(ty) => ty,
        Err(code) => return code.into_response(),
    };

    let state2 = state.clone();
    let mut speedtest_state = state2.speedtest.lock().await;
    if !speedtest_state.should_use_cache(state.config.speedtest.cooldown) {
        speedtest_state.last_measurement = Some(Instant::now());
        speedtest_state.metrics = perform_speedtest(state).await;
    };
    let data = &speedtest_state.metrics[..];

    encode_prometheus_response(&response_type, data)
}

fn format_debug(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct HistogramData {
    pub quantiles: Vec<(f64, f64)>,
    pub mean: f64,
    pub stddev: f64,
    pub sum: f64,
    pub count: usize,
}

use std::{net::SocketAddr, process::ExitCode, sync::Arc, time::Instant};

use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use config::load_config;
use hickory_resolver::TokioAsyncResolver;
use http::{HeaderMap, StatusCode, header};
use lazy_static::lazy_static;
use mime::{
    APPLICATION, APPLICATION_JSON, HTML, JSON, Mime, PLAIN, TEXT, TEXT_HTML, TEXT_HTML_UTF_8,
    TEXT_PLAIN,
};
use tokio::net::TcpListener;

use crate::{
    ping::{PingResult, perform_ping},
    speedtest::{SpeedtestResult, perform_speedtest},
    state::AppState,
};

pub mod config;
pub mod ping;
pub mod prometheus;
pub mod speedtest;
mod state;

lazy_static! {
    static ref TEXT_PLAIN_UTF_8_VERSION_4: Mime =
        "text/plain; version=0.0.4; charset=utf-8".parse().unwrap();
}

pub type Resolver = TokioAsyncResolver;

#[tokio::main]
async fn main() -> ExitCode {
    println!("{}", include_str!("startup-notice.txt"));

    tracing_subscriber::fmt::init();

    let config = match load_config() {
        Ok(c) => c,
        Err(error) => {
            tracing::error!("Could not load config: {error:?}");
            return ExitCode::FAILURE;
        }
    };

    let resolver = match TokioAsyncResolver::tokio_from_system_conf() {
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
            .negotiate(&[TEXT_PLAIN, APPLICATION_JSON])
            .map_err(|code| StatusCode::from_u16(code.as_u16()).unwrap())?
    } else {
        TEXT_PLAIN_UTF_8_VERSION_4.clone()
    };
    if response_type.type_() == TEXT && response_type.get_param("version").is_none() {
        TEXT_PLAIN_UTF_8_VERSION_4.clone_into(&mut response_type);
    } else if response_type == TEXT_HTML {
        response_type = TEXT_HTML_UTF_8;
    }
    Ok(response_type)
}

async fn get_ping(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response<String> {
    let response_type = match negotiate_prometheus_mime(&headers) {
        Ok(ty) => ty,
        Err(code) => {
            return Response::builder()
                .status(code)
                .body(String::new())
                .unwrap();
        }
    };

    let state2 = state.clone();
    let mut ping_state = state2.ping.lock().await;
    if !ping_state.should_use_cache(state.config.ping.cooldown) {
        ping_state.metrics = perform_ping(state).await;
        ping_state.last_measurement = Some(Instant::now());
    };
    let data = &ping_state.metrics[..];

    let response = match (response_type.type_(), response_type.subtype()) {
        (TEXT, PLAIN) => ::prometheus::TextEncoder::new()
            .encode_to_string(&PingResult::range_to_prometheus(&data))
            .expect("Prometheus encoding"),
        (APPLICATION, JSON) => serde_json::to_string_pretty(&data).unwrap(),
        _ => unreachable!(),
    };

    Response::builder()
        .header(header::CONTENT_TYPE, response_type.as_ref())
        .status(StatusCode::OK)
        .body(response)
        .unwrap()
}

async fn get_speedtest(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response<String> {
    let response_type = match negotiate_prometheus_mime(&headers) {
        Ok(ty) => ty,
        Err(code) => {
            return Response::builder()
                .status(code)
                .body(String::new())
                .unwrap();
        }
    };

    let state2 = state.clone();
    let mut speedtest_state = state2.speedtest.lock().await;
    if !speedtest_state.should_use_cache(state.config.speedtest.cooldown) {
        speedtest_state.metrics = perform_speedtest(state).await;
        speedtest_state.last_measurement = Some(Instant::now());
    };
    let data = &speedtest_state.metrics[..];

    let response = match (response_type.type_(), response_type.subtype()) {
        (TEXT, PLAIN) => ::prometheus::TextEncoder::new()
            .encode_to_string(&SpeedtestResult::range_to_prometheus(&data))
            .expect("Prometheus encoding"),
        (APPLICATION, JSON) => serde_json::to_string_pretty(&data).unwrap(),
        _ => unreachable!(),
    };

    Response::builder()
        .header(header::CONTENT_TYPE, response_type.as_ref())
        .status(StatusCode::OK)
        .body(response)
        .unwrap()
}

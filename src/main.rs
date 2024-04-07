use std::{
    error::Error,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    RequestExt, Router,
};
use config::{load_config, Config};
use hickory_resolver::TokioAsyncResolver;
use http::{header, HeaderMap, StatusCode};
use lazy_static::lazy_static;
use mime::{
    Mime, APPLICATION, APPLICATION_JSON, HTML, JSON, PLAIN, TEXT, TEXT_HTML, TEXT_HTML_UTF_8,
    TEXT_PLAIN, TEXT_PLAIN_UTF_8,
};
use rand::Rng;
use serde::Serialize;
use speedtest::{SpeedtestProvider, SpeedtestSummary};
use tokio::{net::TcpListener, task};
use tracing::{info, Level};
use typed_arena::Arena;

use crate::{
    ping::perform_ping,
    prometheus::{ExpositionBuilder, PName},
};

pub mod config;
pub mod ping;
pub mod prometheus;
pub mod speedtest;

lazy_static! {
    static ref TEXT_PLAIN_UTF_8_VERSION_4: Mime =
        "text/plain; version=0.0.4; charset=utf-8".parse().unwrap();
}

pub type Resolver = TokioAsyncResolver;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = load_config()?;
    println!("{}", include_str!("startup-notice.txt"));

    {
        let subscriber = tracing_subscriber::FmtSubscriber::builder()
            // all spans/events with a level higher than TRACE (e.g, debug, info, warn, etc.)
            // will be written to stdout.
            .with_max_level(Level::INFO)
            // completes the builder.
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
    }

    let bind_to = (config.server.address, config.server.port);

    let app = create_router(Arc::new(config));

    let listener = TcpListener::bind(bind_to).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

fn create_router(config: Arc<Config>) -> Router {
    Router::new()
        .route("/", get(get_index))
        .route("/ping", get(get_ping))
        .route("/speedtest", get(get_speedtest))
        .layer(middleware::from_fn(log_traffic))
        .with_state(config)
}

async fn log_traffic(mut req: Request, next: Next) -> Response {
    // Responses usually take a long time, this helps tracking them
    // Format: \x1b[38;2;{rrr};{ggg};{bbb}m{nnnnnnnn}\x1b[0m => max 31 bytes
    let mut id = [0; 31];
    let id = {
        use palette::{hsl::Hsl, FromColor, Srgb};
        use std::io::Write;
        let id_num: u32 = rand::thread_rng().gen();
        let (r, g, b) =
            Srgb::from_color(Hsl::new((id_num % 360) as f32, 1., 0.75)).into_components();
        let mut id_writer = &mut id[..];
        write!(
            id_writer,
            "\x1b[38;2;{r};{g};{b}m{id_num:08X}\x1b[0m",
            r = (r * 255.) as u8,
            g = (g * 255.) as u8,
            b = (b * 255.) as u8
        )
        .unwrap();
        let written = 31 - id_writer.len();
        // SAFETY: All data was fmt written as valid ASCII
        unsafe { std::str::from_utf8_unchecked(&id[..written]) }
    };

    struct Latency(Duration);
    impl std::fmt::Display for Latency {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            if self.0.as_secs() == 0 {
                let ns = self.0.subsec_nanos();
                let ms = ns / 1_000_000;
                let frac_ms = (ns / 1000) % 1000;
                write!(f, "{ms}.{frac_ms:03}ms")
            } else {
                write!(f, "{:.3}s", self.0.as_secs_f32())
            }
        }
    }

    let ConnectInfo(source) = req
        .extract_parts::<ConnectInfo<SocketAddr>>()
        .await
        .unwrap();
    let method = req.method();
    let path = req.uri().path();
    info!(%id, %method, path, %source, "Request");

    let start = Instant::now();
    let res = next.run(req).await;
    let latency = Latency(start.elapsed());

    let status = res.status().as_u16();
    info!(%id, status, %latency, "Response");
    res
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
        )
            .into_response(),

        (TEXT, PLAIN) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime::TEXT_PLAIN_UTF_8.as_ref())],
            include_str!("public/index.txt"),
        )
            .into_response(),

        (TEXT, HTML) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime::TEXT_HTML_UTF_8.as_ref())],
            include_str!("public/index.html"),
        )
            .into_response(),

        _ => unreachable!(),
    }
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

async fn get_ping(State(config): State<Arc<Config>>, headers: HeaderMap) -> Response<String> {
    let response_type = match negotiate_prometheus_mime(&headers) {
        Ok(ty) => ty,
        Err(code) => {
            return Response::builder()
                .status(code)
                .body(String::new())
                .unwrap()
        }
    };

    let data = match perform_ping(config).await {
        Ok(data) => data,
        Err(error) => return error_to_500(&error),
    };

    let mut response = String::new();
    match (response_type.type_(), response_type.subtype()) {
        (TEXT, PLAIN) => {
            use std::fmt::Write as _;
            let alloc = Arena::new();
            let mut builder = ExpositionBuilder::new(&alloc);
            for result in data {
                result.write_prometheus(&mut builder);
            }
            write!(response, "{builder}").unwrap();
        }
        (APPLICATION, JSON) => {
            response = serde_json::to_string_pretty(&data).unwrap();
        }
        _ => unreachable!(),
    }

    Response::builder()
        .header(header::CONTENT_TYPE, response_type.as_ref())
        .status(StatusCode::OK)
        .body(response)
        .unwrap()
}

async fn get_speedtest(State(config): State<Arc<Config>>, headers: HeaderMap) -> Response<String> {
    let response_type = match negotiate_prometheus_mime(&headers) {
        Ok(ty) => ty,
        Err(code) => {
            return Response::builder()
                .status(code)
                .body(String::new())
                .unwrap()
        }
    };

    let download_data = match config.speedtest.provider.measure_download().await {
        Ok(rates) => {
            let config = config.clone();
            task::spawn_blocking(move || {
                SpeedtestSummary::digest_data(rates, &config.speedtest.quantiles)
            })
        }
        Err(error) => return error_to_500(&error),
    };

    let upload_data = match config.speedtest.provider.measure_upload().await {
        Ok(rates) => {
            //let config = config.clone();
            task::spawn_blocking(move || {
                SpeedtestSummary::digest_data(rates, &config.speedtest.quantiles)
            })
        }
        Err(error) => return error_to_500(&error),
    };

    let download_data = download_data.await.unwrap();
    let upload_data = upload_data.await.unwrap();

    let mut response = String::new();
    match (response_type.type_(), response_type.subtype()) {
        (TEXT, PLAIN) => {
            use std::fmt::Write as _;
            let alloc = Arena::new();
            let mut builder = ExpositionBuilder::new(&alloc);
            let direction = PName::new("direction").unwrap();
            builder.with_label(direction, "down", |builder| {
                download_data.write_prometheus(builder);
            });
            builder.with_label(direction, "up", |builder| {
                upload_data.write_prometheus(builder);
            });
            write!(response, "{builder}").unwrap();
        }
        (APPLICATION, JSON) => {
            #[derive(Serialize)]
            struct Data<'a> {
                down: &'a SpeedtestSummary,
                up: &'a SpeedtestSummary,
            }

            response = serde_json::to_string_pretty(&Data {
                down: &download_data,
                up: &upload_data,
            })
            .unwrap();
        }
        _ => unreachable!(),
    }

    Response::builder()
        .header(header::CONTENT_TYPE, response_type.as_ref())
        .status(StatusCode::OK)
        .body(response)
        .unwrap()
}

#[cold]
fn error_to_500(error: &dyn Error) -> Response<String> {
    Response::builder()
        .header(header::CONTENT_TYPE, TEXT_PLAIN_UTF_8.as_ref())
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(error.to_string())
        .unwrap()
}

use std::{error::Error, sync::Arc};

use axum::{extract::State, response::Html, routing::get, Router};
use config::{load_config, Config};
use hickory_resolver::TokioAsyncResolver;
use http::{header, HeaderMap, Response, StatusCode};
use lazy_static::lazy_static;
use mime::{
    Mime, APPLICATION, APPLICATION_JSON, JSON, PLAIN, TEXT, TEXT_HTML, TEXT_HTML_UTF_8, TEXT_PLAIN,
    TEXT_PLAIN_UTF_8,
};
use serde::Serialize;
use speedtest::SpeedtestSummary;
use tokio::net::TcpListener;
use tracing::Level;
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

    let app = Router::new()
        .route("/", get(get_index))
        .route("/ping", get(get_ping))
        .route("/speedtest", get(get_speedtest))
        .with_state(Arc::new(config));

    let listener = TcpListener::bind(bind_to).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_index() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

fn negotiate_mime(headers: &HeaderMap) -> Result<Mime, StatusCode> {
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
    let response_type = match negotiate_mime(&headers) {
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
    let response_type = match negotiate_mime(&headers) {
        Ok(ty) => ty,
        Err(code) => {
            return Response::builder()
                .status(code)
                .body(String::new())
                .unwrap()
        }
    };

    let download_data = match config
        .speedtest
        .provider
        .measure_download(config.clone())
        .await
    {
        Ok(rates) => SpeedtestSummary::digest_data(rates, &config.speedtest.quantiles),
        Err(error) => return error_to_500(&error),
    };

    let upload_data = match config
        .speedtest
        .provider
        .measure_upload(config.clone())
        .await
    {
        Ok(rates) => SpeedtestSummary::digest_data(rates, &config.speedtest.quantiles),
        Err(error) => return error_to_500(&error),
    };

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

use std::net::SocketAddr;

use axum::http::HeaderMap;

pub(crate) mod cache;
pub(crate) mod database;
pub(crate) mod endpoints;
pub(crate) mod error;
pub(crate) mod foods;
pub(crate) mod health;

fn parse_client_ip(proxy_addr: &SocketAddr, headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.split(",").next())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| format!("proxy: {}", proxy_addr.ip()))
}

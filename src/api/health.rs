use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use axum::{
    Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
};
use chrono::{FixedOffset, Utc};
use reqwest::ClientBuilder;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::debug;

use crate::{SharedState, api::parse_client_ip};

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct ServerHealth {
    name: &'static str,
    version: &'static str,
    status: &'static str,
    details: ServerHealthDetails,
    documentation: &'static str,
    source_code: &'static str,
    last_updated: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct ServerHealthDetails {
    internet_connection: bool,
    database_functionality: bool,
}

// Cargo bize environment üzerinden sürümü sağlıyor, manuel girmeye gerek yok
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) async fn health(
    State(shared_state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Json<ServerHealth> {
    let timestamp = {
        let utc_time = Utc::now();
        let turkish_offset = FixedOffset::east_opt(3 * 3600).unwrap(); // +3 saat
        utc_time.with_timezone(&turkish_offset).to_rfc3339() // ör: 2025-09-13T21:42:35.785219+03:00 (ISO 8601)
    };

    // URL'lere klon atmadan yaparsak Mutex'i serbest bırakmadığımız için config'i blokluyor, yani diğer bağlantıları bloklamaması için urlleri klonluyoruz
    // Zaten bir URL'ye ping atmak birkaç yüz ms sürdüğü için buradaki klon ne RAM ne de hız olarak önemli bir etkiye sebep olacak
    let urls = &shared_state
        .config
        .lock()
        .await
        .api
        .health_internet_check_urls
        .clone();
    let is_connected_to_internet = check_internet(urls).await;
    let is_database_functional = check_database(&*shared_state.api_db.lock().await).await;

    let health = ServerHealth {
        name: "besinveri-api",
        version: VERSION,
        status: if is_connected_to_internet && is_database_functional {
            "healthy"
        } else {
            "unhealthy"
        },
        details: ServerHealthDetails {
            internet_connection: is_connected_to_internet,
            database_functionality: is_database_functional,
        },
        documentation: "https://github.com/karahanbuhan/besinveri-api",
        source_code: "https://github.com/karahanbuhan/besinveri-api",
        last_updated: timestamp,
    };

    debug!(
        "GET /health: ({}), {}",
        health.status,
        parse_client_ip(&addr, &headers)
    );
    Json(health)
}

async fn check_database(pool: &SqlitePool) -> bool {
    sqlx::query("SELECT 1").fetch_one(pool).await.is_ok()
}

async fn check_internet(urls: &Vec<String>) -> bool {
    let client = match ClientBuilder::new()
        .timeout(Duration::from_secs(3)) // 3 saniyeden fazla beklemiyoruz, bu kadar uzun bir bağlantı süresi zaten bağlantıda bir sorun olduğuna işarettir
        .local_address(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))) // IPv4 ile bağlanmak istiyoruz, gelecekte güncellenebilir
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };

    // Sadece bir URL'ye bağlantının başarılı olması bizim için yeterli
    for url in urls {
        if client
            .get(url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return true;
        }
    }

    false
}

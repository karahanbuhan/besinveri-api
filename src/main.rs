use std::{net::SocketAddr, str::FromStr, sync::Arc};

use anyhow::Error;
use axum::{
    Router, ServiceExt,
    extract::Request,
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use axum_client_ip::ClientIpSource;
use axum_governor::GovernorLayer;
use axum_helmet::{Helmet, HelmetLayer};
use lazy_limit::{Duration, RuleConfig, init_rate_limiter};
use moka::future::Cache;
use real::RealIpLayer;
use reqwest::{Method, header};
use sqlx::{Pool, Sqlite};
use tokio::{net::TcpListener, sync::Mutex};
use tower::Layer;
use tower_http::{cors::CorsLayer, normalize_path::NormalizePathLayer};
use tracing::{debug, info};

use crate::core::config::Config;

mod api;
mod core;

// Veritabanı ve config'i, tüm handlerlar içinde kullanabilmek için bir shared_state oluşturuyoruz, cache de dahil
#[derive(Clone)]
struct SharedState {
    api_db: Arc<Mutex<Pool<Sqlite>>>,
    config: Arc<Mutex<Config>>,
    cache: Cache<String, String>, // URL -> JSON şeklinde caching yapacağız
}

impl SharedState {
    async fn new() -> Result<Self, Error> {
        let api_db = Arc::new(Mutex::new(api::database::connect_database().await?));
        let config = Arc::new(Mutex::new(core::config::load_config_with_defaults()?));

        let cache_capacity = config.lock().await.core.cache_capacity;
        let cache = Cache::builder()
            .max_capacity(cache_capacity)
            .time_to_live(std::time::Duration::from_secs(10 * 60))
            .build();

        Ok(Self {
            api_db,
            config,
            cache,
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Bu state içinde veritabanı, config ve cache'i barındırıyor. Diğer route'lardan erişmek için kullanıyoruz asenkron olarak
    let shared_state = SharedState::new().await?;

    // http(s)://alanadi.com/API/NEST/PATH -> Bu şekilde girildiğinde /API/NEST/PATH'i kullanacağız nest için
    // Scope içine açıyorum ownership sorununu düzeltmek için, ayrıca String kullanmamız gerekecek referans kullanamayız burada
    let api_path: String = {
        let config_guard = shared_state.config.lock().await;
        config_guard
            .api
            .base_url
            .replace("://", "") // Kesme işaretlerini istemiyoru başlangıçtaki
            .split_once('/')
            .map(|(_before, after)| format!("/{}", after))
            .unwrap_or("/".to_owned())
    };

    // Config'den trace seviyesini alıp kullanıyoruz, bunun için yine bir MutexGuard kullandık.
    {
        let config_guard = shared_state.config.lock().await;
        let tracing_level = tracing::Level::from_str(&config_guard.core.tracing_level)
            .unwrap_or(tracing::Level::TRACE);
        tracing_subscriber::fmt()
            .with_max_level(tracing_level)
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
            .init();
    }

    debug!("Rate limiter başlatılıyor");
    // Lazy-limit ile rate-limit ayarlıyoruz, şimdilik basit bir sistem kullanıyoruz; 1 saniyede maksimum 5 istek.
    // Gelecekte kova mantığına geçilebilir ama şimdilik bu sistemin yeterli olması gerekli
    init_rate_limiter!(
        default: RuleConfig::new(Duration::Seconds(1), 5),
        max_memory: Some(64 * 1024 * 1024) // 64MB maksimum bellek
    )
    .await;

    debug!("BesinVeri API hazırlanıyor");
    // Nest'in içine boş path yazarsak Axum sorun çıkartıyor o yüzden böyle yapıyoruz
    let router = if api_path == "/" {
        api_router(shared_state)
    } else {
        Router::new().nest(&api_path, api_router(shared_state))
    };

    debug!("CORS mekanizması hazırlanıyor");
    // Web Uygulamalarda tarayıcıların sorun çıkartmaması için CORS header mekanizmasını da ekliyoruz
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([Method::GET])
        .allow_headers(tower_http::cors::Any)
        .max_age(std::time::Duration::from_secs(3600));

    debug!("Trailing slash çözülüyor");
    // trim_trailing_slash ile /api/ -> /api şeklinde düzeltiyoruz aksi takdirde routelar çalışmıyor, ayrıca IP adreslerine de ihtiyacımız var rate limit için, connect info ayarlıyoruz
    let router = ServiceExt::<Request>::into_make_service_with_connect_info::<SocketAddr>(
        NormalizePathLayer::trim_trailing_slash().layer(router.layer(cors)),
    );

    info!("BesinVeri API aktif!");
    axum::serve(TcpListener::bind("0.0.0.0:8099").await?, router).await?;
    info!("BesinVeri API pasif!");

    Ok(())
}

fn api_router(shared_state: SharedState) -> Router {
    Router::new()
        .route("/", get(api::endpoints::endpoints))
        .route("/health", get(api::health::health))
        .route("/food/{slug}", get(api::foods::food))
        .route("/foods", get(api::foods::foods))
        .route("/foods/list", get(api::foods::foods_list))
        .route("/foods/search", get(api::foods::foods_search))
        .route("/tags", get(api::foods::tags_list))
        .with_state(shared_state.clone())
        .fallback(api::error::APIError::not_found_handler)
        .route_layer(middleware::from_fn_with_state(
            shared_state.clone(),
            |state, request, next| api::cache::cache_middleware(state, request, next),
        ))
        .layer(
            tower::ServiceBuilder::new()
                .layer(ClientIpSource::RightmostXForwardedFor.into_extension()) // Caddy gibi reverse proxy yazılımlarından doğru istemci IP'sini almak için gerekli
                .layer(RealIpLayer::default()) // Governor'dan önce kurulmalı
                .layer(GovernorLayer::default()), // Bu katman rate limiter için
        )
        .layer(HelmetLayer::new(
            // Özellikle başkalarının iframe içinde API'yi kullanamaması için bu katmanı ekliyoruz
            Helmet::new()
                .add(helmet_core::XContentTypeOptions::nosniff())
                .add(helmet_core::XFrameOptions::deny())
                .add(helmet_core::XXSSProtection::on().mode_block()) // Eski tarayıcılar için gerekli
                .add(
                    helmet_core::ContentSecurityPolicy::new()
                        .default_src(vec!["'none'"])
                        .script_src(vec!["'self'"])
                        .style_src(vec!["'self'", "'unsafe-inline"])
                        .img_src(vec!["'self'", "data:"])
                        .connect_src(vec!["'self'"])
                        .frame_ancestors(vec!["'none"]),
                )
                .add(helmet_core::ReferrerPolicy::no_referrer()),
        ))
        .layer(middleware::from_fn(api::error::handle_axum_rejections)) // Bu da axum'un kendi hataları için, özellikle deserializasyon gibi hatalar için JSON çevirici
        .layer(middleware::from_fn(utf8_header_middleware)) // Content Type header'ına UTF8 eklemek için bu middleware'i kullanıyoruz
}

async fn utf8_header_middleware(request: Request, next: Next) -> Response {
    // Bu middleware'i daha gömülü yapabiliriz gelecekte performansı arttırmak için mevcut cache/route mekanizmalarına
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if let Some(content_type) = headers.get(header::CONTENT_TYPE) {
        if let Ok(content_type) = content_type.to_str() {
            // Axum kendisi eklemiyor ama yine de bir teksir durumu olmaması için kontrol edelim charset var mı diye
            if !content_type.to_lowercase().contains("charset") {
                let content_type = format!("{}; charset=utf-8", content_type);
                if let Ok(new_val) = header::HeaderValue::from_str(&content_type) {
                    headers.insert(header::CONTENT_TYPE, new_val);
                }
            }
        }
    }
    response
}

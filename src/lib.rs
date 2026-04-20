#![allow(clippy::too_many_arguments)]

pub mod config;
pub mod db;
pub mod db_pool;
pub mod middleware;
pub mod routes;
pub mod services;
pub mod utils;

use actix_web::{web, App, HttpResponse, HttpServer};
use solana_sdk::signer::Signer;
use tokio::time::{timeout, Duration};

use crate::config::AppConfig;
use crate::middleware::security_headers;
use crate::services::airbills;
use crate::services::exchange_rate::format_usdc;
use crate::services::solana_rpc::SolanaRpc;

pub async fn run() -> std::io::Result<()> {
    let config = AppConfig::from_env();

    let pool = db_pool::create_pool(
        &config.database_url,
        &config.database_ssl,
        config.database_pool_max_size,
    );

    if let Err(e) = db::run_startup_migrations(&pool).await {
        panic!("Failed to run startup migrations: {e}");
    }

    let redis = redis::Client::open(config.redis_url.clone()).expect("Failed to connect to Redis");

    let rpc = SolanaRpc::new(&config.solana_rpc_url);

    let bind_addr = format!("{}:{}", config.host, config.port);

    log::info!(
        "\n\
         ╔═══════════════════════════════════════════╗\n\
         ║      Payce USSD Server (Rust)             ║\n\
         ║    USSD + USDC Payments on Solana         ║\n\
         ╠═══════════════════════════════════════════╣\n\
         ║  Server:  http://{}                       ║\n\
         ║  USSD:    POST /ussd/callback/{{secret}}  ║\n\
         ║  Health:  GET  /health                    ║\n\
         ║  Network: {:31}                           ║\n\
         ║  Fee payer: {}                            ║\n\
         ║  Gas fee:   {}/tx                         ║\n\
         ╚═══════════════════════════════════════════╝",
        bind_addr,
        config.solana_network,
        config.fee_payer.pubkey(),
        format_usdc(config.gas_fee_usdc as f64 / 1_000_000.0),
    );
    log::info!(
        "Postgres TLS mode: {} | AT_SHORTCODE={} | SMS URL={}",
        config.database_ssl,
        config.at_shortcode.as_deref().unwrap_or("(unset)"),
        config.at_messaging_url
    );

    match timeout(Duration::from_secs(8), airbills::businesses_me(&config)).await {
        Ok(Err(e)) => {
            log::warn!("Airbills businesses/me failed: {e} (vendor may still work with secretkey).")
        }
        Err(_) => log::warn!("Airbills businesses/me timed out."),
        Ok(Ok(_)) => {}
    }
    match timeout(Duration::from_secs(8), airbills::list_tokens(&config)).await {
        Ok(Err(e)) => log::warn!("Airbills /tokens failed: {e}"),
        Err(_) => log::warn!("Airbills /tokens timed out."),
        Ok(Ok(_)) => {}
    }

    let t = Duration::from_secs(8);
    let (r_elect, r_bet, r_cab) = tokio::join!(
        timeout(t, airbills::list_elect(&config)),
        timeout(t, airbills::list_bet(&config)),
        timeout(t, airbills::list_cable(&config)),
    );
    if let Ok(Err(e)) = r_elect {
        log::warn!("Airbills list/elect failed: {e}");
    } else if r_elect.is_err() {
        log::warn!("Airbills list/elect timed out.");
    }
    if let Ok(Err(e)) = r_bet {
        log::warn!("Airbills list/bet failed: {e}");
    } else if r_bet.is_err() {
        log::warn!("Airbills list/bet timed out.");
    }
    if let Ok(Err(e)) = r_cab {
        log::warn!("Airbills list/cable failed: {e}");
    } else if r_cab.is_err() {
        log::warn!("Airbills list/cable timed out.");
    }

    let config_data = web::Data::new(config);
    let pool_data = web::Data::new(pool);
    let redis_data = web::Data::new(redis);
    let rpc_data = web::Data::new(rpc);

    HttpServer::new(move || {
        App::new()
            .wrap(security_headers::secure_default_headers(&config_data))
            .app_data(web::FormConfig::default().limit(16_384))
            .app_data(config_data.clone())
            .app_data(pool_data.clone())
            .app_data(redis_data.clone())
            .app_data(rpc_data.clone())
            .route("/", web::get().to(root))
            .route("/", web::post().to(routes::ussd::ussd_callback))
            // Path-based callback secret (register this URL in Africa's Talking).
            // Must be registered before the bare `/ussd/callback` route.
            .route(
                "/ussd/callback/{at_key}",
                web::post().to(routes::ussd::ussd_callback),
            )
            .route(
                "/ussd/callback",
                web::post().to(routes::ussd::ussd_callback),
            )
            .route("/health", web::get().to(routes::merchant::health))
            .route("/api/health", web::get().to(routes::merchant::health))
            .route(
                "/api/merchants/{code}",
                web::get().to(routes::merchant::lookup_merchant),
            )
            .route(
                "/api/paj/webhooks/{order_id}",
                web::post().to(routes::paj::paj_webhook),
            )
            .route("/api/paj/onramp", web::post().to(routes::paj::paj_onramp))
            .route("/api/paj/offramp", web::post().to(routes::paj::paj_offramp))
    })
    .bind(&bind_addr)?
    .run()
    .await
}

async fn root() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({
        "service": "Payce",
        "runtime": "Rust/Actix",
        "health": "/health",
        "ussdCallback": "POST /ussd/callback",
        "merchantLookup": "GET /api/merchants/:code",
        "pajOnramp": "POST /api/paj/onramp",
        "pajOfframp": "POST /api/paj/offramp",
        "pajWebhook": "POST /api/paj/webhooks/:orderId",
    }))
}

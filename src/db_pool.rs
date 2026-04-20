use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
use tokio_postgres::NoTls;

pub fn create_pool(database_url: &str, ssl_mode: &str, max_size: usize) -> Pool {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    cfg.pool = Some(deadpool_postgres::PoolConfig::new(max_size));

    let mode = ssl_mode.trim().to_ascii_lowercase();
    match mode.as_str() {
        "require" | "true" | "1" | "yes" => {
            let connector = TlsConnector::builder()
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true)
                .build()
                .expect("DATABASE_SSL=require: failed to build native TLS connector");
            let tls = MakeTlsConnector::new(connector);
            cfg.create_pool(Some(Runtime::Tokio1), tls)
                .expect("Failed to create Postgres pool (TLS)")
        }
        "verify-full" => {
            let connector = TlsConnector::builder()
                .build()
                .expect("DATABASE_SSL=verify-full: failed to build native TLS connector");
            let tls = MakeTlsConnector::new(connector);
            cfg.create_pool(Some(Runtime::Tokio1), tls)
                .expect("Failed to create Postgres pool (TLS verify-full)")
        }
        "disable" | "false" | "0" | "no" | "" => cfg
            .create_pool(Some(Runtime::Tokio1), NoTls)
            .expect("Failed to create Postgres pool (no TLS)"),
        other => panic!(
            "DATABASE_SSL must be `disable` (local dev), `require` (TLS, no cert verify — standard for managed Postgres), or `verify-full` (TLS + cert verify). Got: {other}"
        ),
    }
}

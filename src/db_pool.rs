use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use native_tls::TlsConnector;
use postgres_native_tls::MakeTlsConnector;
use tokio_postgres::NoTls;

pub fn create_pool(database_url: &str, ssl_mode: &str) -> Pool {
    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });

    let mode = ssl_mode.trim().to_ascii_lowercase();
    match mode.as_str() {
        "require" | "verify-full" | "true" | "1" | "yes" => {
            let connector = TlsConnector::builder()
                .build()
                .expect("DATABASE_SSL=require: failed to build native TLS connector");
            let tls = MakeTlsConnector::new(connector);
            cfg.create_pool(Some(Runtime::Tokio1), tls)
                .expect("Failed to create Postgres pool (TLS)")
        }
        "disable" | "false" | "0" | "no" | "" => cfg
            .create_pool(Some(Runtime::Tokio1), NoTls)
            .expect("Failed to create Postgres pool (no TLS)"),
        other => panic!(
            "DATABASE_SSL must be `disable` (local dev) or `require` (TLS to Postgres). Got: {other}"
        ),
    }
}

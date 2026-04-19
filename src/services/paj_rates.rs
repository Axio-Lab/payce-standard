//! PAJ public FX rates (`GET /pub/rate`) — no session.
//! Used for indicative NGN conversion on SOL legs (off-ramp NGN-per-USD).

use std::sync::Mutex;
use std::time::Instant;

use serde::Deserialize;

use crate::config::AppConfig;

#[derive(Debug, Clone, Copy)]
pub struct PajNgnPerUsd {
    pub on_ramp: f64,
    pub off_ramp: f64,
}

#[derive(Debug, Deserialize)]
struct RateBlock {
    rate: f64,
}

#[derive(Debug, Deserialize)]
struct AllRatesBody {
    #[serde(rename = "onRampRate")]
    on_ramp_rate: RateBlock,
    #[serde(rename = "offRampRate")]
    off_ramp_rate: RateBlock,
}

struct Cached {
    rates: PajNgnPerUsd,
    fetched_at: Instant,
}

static CACHE: std::sync::LazyLock<Mutex<Option<Cached>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

pub fn parse_rates_body(body: &str) -> Result<PajNgnPerUsd, String> {
    let parsed: AllRatesBody = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let rates = PajNgnPerUsd {
        on_ramp: parsed.on_ramp_rate.rate,
        off_ramp: parsed.off_ramp_rate.rate,
    };
    if rates.on_ramp > 0.0 && rates.off_ramp > 0.0 {
        Ok(rates)
    } else {
        Err("non-positive rate(s)".into())
    }
}

pub async fn get_paj_ngn_per_usd_cached(config: &AppConfig) -> Option<PajNgnPerUsd> {
    let base = config.paj_base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    let ttl = config.exchange_rate_cache_ttl_secs.max(30);

    {
        let g = CACHE.lock().unwrap();
        if let Some(ref c) = *g {
            if c.fetched_at.elapsed().as_secs() < ttl {
                return Some(c.rates);
            }
        }
    }

    let url = format!("{base}/pub/rate");
    let resp = match config.http.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[PAJ rate] HTTP: {e}");
            return CACHE.lock().unwrap().as_ref().map(|c| c.rates);
        }
    };
    let status = resp.status();
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => {
            log::warn!("[PAJ rate] body: {e}");
            return CACHE.lock().unwrap().as_ref().map(|c| c.rates);
        }
    };
    if !status.is_success() {
        log::warn!(
            "[PAJ rate] {status}: {}",
            body.chars().take(200).collect::<String>()
        );
        return CACHE.lock().unwrap().as_ref().map(|c| c.rates);
    }
    let rates = match parse_rates_body(&body) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("[PAJ rate] parse: {e}");
            return CACHE.lock().unwrap().as_ref().map(|c| c.rates);
        }
    };
    let mut g = CACHE.lock().unwrap();
    *g = Some(Cached {
        rates,
        fetched_at: Instant::now(),
    });
    Some(rates)
}

use std::sync::Mutex;
use std::time::Instant;

use crate::config::AppConfig;

struct CachedRate {
    rate: f64,
    fetched_at: Instant,
}

static CACHE: std::sync::LazyLock<Mutex<Option<CachedRate>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

pub async fn get_usd_to_ngn_rate(config: &AppConfig) -> f64 {
    let ttl = config.exchange_rate_cache_ttl_secs;
    {
        let cache = CACHE.lock().unwrap();
        if let Some(ref c) = *cache {
            if c.fetched_at.elapsed().as_secs() < ttl {
                return c.rate;
            }
        }
    }

    match fetch_rate(
        &config.http,
        &config.exchange_rate_api_url,
        &config.exchange_rate_quote_currency,
    )
    .await
    {
        Ok(rate) => {
            let mut cache = CACHE.lock().unwrap();
            *cache = Some(CachedRate {
                rate,
                fetched_at: Instant::now(),
            });
            rate
        }
        Err(e) => {
            log::warn!("Exchange rate fetch failed: {e}");
            let cache = CACHE.lock().unwrap();
            if let Some(ref c) = *cache {
                return c.rate;
            }
            config.exchange_rate_fallback_ngn
        }
    }
}

async fn fetch_rate(
    http: &reqwest::Client,
    url: &str,
    quote_currency: &str,
) -> Result<f64, String> {
    let resp: serde_json::Value = http
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    resp.get("conversion_rates")
        .or_else(|| resp.get("rates"))
        .and_then(|r| r.get(quote_currency))
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("{quote_currency} rate not found in response"))
}

pub fn usd_to_ngn(usd: f64, rate: f64) -> f64 {
    (usd * rate * 100.0).round() / 100.0
}

pub fn ngn_to_usd(ngn: f64, rate: f64) -> f64 {
    (ngn / rate * 1_000_000.0).round() / 1_000_000.0
}

fn insert_commas_unsigned(digits: &str) -> String {
    if digits.is_empty() {
        return "0".into();
    }
    let n = digits.len();
    let lead = match n % 3 {
        0 => 3,
        r => r,
    };
    let mut out = String::with_capacity(n + n / 3);
    out.push_str(&digits[..lead]);
    let mut pos = lead;
    while pos < n {
        out.push(',');
        out.push_str(&digits[pos..pos + 3]);
        pos += 3;
    }
    out
}

fn insert_commas_u64(n: u64) -> String {
    insert_commas_unsigned(&n.to_string())
}

pub fn format_ngn(amount: f64) -> String {
    let neg = amount < 0.0;
    let a = amount.abs();
    let cents = (a * 100.0).round() as u64;
    let int_part = cents / 100;
    let frac = (cents % 100) as u32;
    let body = if frac == 0 {
        format!("₦{}", insert_commas_u64(int_part))
    } else {
        format!("₦{}.{frac:02}", insert_commas_u64(int_part))
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

pub fn format_balance_usdc_sol(ngn_amount: f64, usdc_amount: f64, sol_amount: f64) -> String {
    format!(
        "{} (~{}, {})",
        format_ngn(ngn_amount),
        format_usdc(usdc_amount),
        format_sol(sol_amount)
    )
}

pub fn format_balance_multi_stable_sol(
    rate: f64,
    stables: &[(String, f64)],
    sol_amount: f64,
) -> String {
    let parts: Vec<String> = stables
        .iter()
        .map(|(code, amt)| {
            let ngn = usd_to_ngn(*amt, rate);
            format!(
                "{} {} (~{})",
                code,
                format_stable_qty_trimmed(*amt),
                format_ngn(ngn)
            )
        })
        .collect();
    format!("{} | {}", parts.join(" | "), format_sol(sol_amount))
}

pub fn format_sol(sol: f64) -> String {
    let neg = sol < 0.0;
    let a = sol.abs();
    let lamports = (a * 1_000_000_000.0).round() as u64;
    let whole = lamports / 1_000_000_000;
    let frac = lamports % 1_000_000_000;
    let frac_str = if frac == 0 {
        String::new()
    } else {
        let mut s = format!("{:09}", frac);
        while s.ends_with('0') {
            s.pop();
        }
        format!(".{s}")
    };
    let body = format!("{}{} SOL", insert_commas_u64(whole), frac_str);
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

pub fn format_usdc(amount: f64) -> String {
    let neg = amount < 0.0;
    let a = amount.abs();
    let cents = (a * 100.0).round() as u64;
    let dollars = cents / 100;
    let frac = (cents % 100) as u32;
    let body = format!("${}.{:02}", insert_commas_u64(dollars), frac);
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

pub fn format_stable_qty_trimmed(amount: f64) -> String {
    let neg = amount < 0.0;
    let a = amount.abs();
    let cents = (a * 100.0).round() as u64;
    let whole = cents / 100;
    let frac = (cents % 100) as u32;
    let body = format!("{}.{:02}", insert_commas_u64(whole), frac);
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

pub fn format_stable_amount_with_code(amount: f64, code: &str) -> String {
    format!("≈{} {}", format_stable_qty_trimmed(amount), code)
}

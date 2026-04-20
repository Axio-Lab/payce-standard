use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use crate::utils::validation::{
    assert_public_https_url, assert_wallet_encryption_key, assert_wallet_master_seed,
};

#[derive(Clone, Debug)]
pub struct StableCoinMint {
    pub code: String,
    pub mint: Pubkey,
}

#[derive(Clone)]
pub struct AppConfig {
    pub port: u16,
    pub host: String,
    pub node_env: String,
    pub database_url: String,
    pub database_ssl: String,
    pub redis_url: String,
    pub solana_rpc_url: String,
    pub solana_network: String,
    pub solana_explorer_base: String,
    pub usdc_mint: Pubkey,
    pub sol_mint: Pubkey,
    pub stable_coins: Vec<StableCoinMint>,
    pub wallet_master_seed: String,
    pub wallet_encryption_key: String,
    pub at_api_key: String,
    pub at_username: String,
    pub at_sender_id: String,
    pub at_shortcode: Option<String>,
    pub at_messaging_url: String,
    pub at_callback_allowed_ips: Vec<String>,
    pub at_callback_url_key: String,
    pub trust_proxy_xff: bool,
    pub exchange_rate_api_url: String,
    pub exchange_rate_cache_ttl_secs: u64,
    pub exchange_rate_fallback_ngn: f64,
    pub exchange_rate_quote_currency: String,
    pub fee_payer: Arc<Keypair>,
    pub gas_fee_usdc: u64,
    pub max_pin_attempts: u32,
    pub lockout_minutes: u64,
    pub tier1_daily_ngn: f64,
    pub tier2_daily_ngn: f64,
    pub rate_limit_window_seconds: i64,
    pub rate_limit_max_per_phone: u32,
    pub rate_limit_max_per_ip: u32,
    pub airbills_base_url: String,
    pub airbills_api_key: String,
    pub jupiter_api_key: String,
    pub jupiter_swap_base_url: String,
    pub jupiter_lend_base_url: String,
    pub jupiter_referral_account: String,
    pub jupiter_referral_fee_bps: u32,
    pub payce_earn_fee_bps: u32,
    pub paj_api_key: String,
    pub paj_base_url: String,
    pub payce_public_base_url: String,
    pub paj_webhook_secret: String,
    pub payce_internal_api_key: String,
    pub resend_api_key: String,
    pub resend_from: String,
    pub paj_ramp_business_usdc_fee: f64,
    pub database_pool_max_size: usize,
    pub http: reqwest::Client,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let fee_payer_b58 = require_env("FEE_PAYER_PRIVATE_KEY");
        let fee_payer_bytes = bs58::decode(fee_payer_b58.trim())
            .into_vec()
            .expect("FEE_PAYER_PRIVATE_KEY is not valid base58");
        let fee_payer = Keypair::from_bytes(&fee_payer_bytes)
            .expect("FEE_PAYER_PRIVATE_KEY is not a valid 64-byte Solana keypair");

        let gas_fee_usdc: u64 = require_parse("GAS_FEE_USDC");

        let at_username = require_env("AT_USERNAME");

        let at_messaging_url = {
            let override_raw = require_env("AT_SMS_API_URL");
            let override_trimmed = override_raw.trim();
            if !override_trimmed.is_empty() {
                let u = override_trimmed.to_string();
                assert_public_https_url("AT_SMS_API_URL", &u);
                u
            } else {
                let sandbox_url = require_env("AT_SMS_SANDBOX_URL");
                let production_url = require_env("AT_SMS_PRODUCTION_URL");
                assert_public_https_url("AT_SMS_SANDBOX_URL", &sandbox_url);
                assert_public_https_url("AT_SMS_PRODUCTION_URL", &production_url);
                if at_username.trim() == "sandbox" {
                    sandbox_url
                } else {
                    production_url
                }
            }
        };

        let callback_ips_raw = require_env("AT_CALLBACK_ALLOWED_IPS");
        let at_callback_allowed_ips = parse_csv_nonempty(&callback_ips_raw);
        assert!(
            !at_callback_allowed_ips.is_empty(),
            "AT_CALLBACK_ALLOWED_IPS must list at least one IP (comma-separated)"
        );

        // Optional shared secret accepted as `?key=…` on the AT callback URL.
        // Lets AT (which only lets you set a URL, no custom headers) authenticate
        // without relying on IP allowlists. Generate with `openssl rand -hex 32`.
        let at_callback_url_key = std::env::var("AT_CALLBACK_URL_KEY")
            .unwrap_or_default()
            .trim()
            .to_string();

        let trust_proxy_xff = std::env::var("TRUST_PROXY_XFF")
            .ok()
            .map(|s| matches!(s.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
            .unwrap_or(false);

        let usdc_mint = Pubkey::from_str(require_env("USDC_MINT_ADDRESS").trim())
            .expect("Invalid USDC_MINT_ADDRESS");
        let usdt_mint = Pubkey::from_str(require_env("USDT_MINT_ADDRESS").trim())
            .expect("Invalid USDT_MINT_ADDRESS");
        let usdg_mint = Pubkey::from_str(require_env("USDG_MINT_ADDRESS").trim())
            .expect("Invalid USDG_MINT_ADDRESS");
        assert_ne!(
            usdc_mint, usdt_mint,
            "USDC_MINT_ADDRESS and USDT_MINT_ADDRESS must differ"
        );
        assert_ne!(
            usdc_mint, usdg_mint,
            "USDC_MINT_ADDRESS and USDG_MINT_ADDRESS must differ"
        );
        assert_ne!(
            usdt_mint, usdg_mint,
            "USDT_MINT_ADDRESS and USDG_MINT_ADDRESS must differ"
        );
        let sol_mint = Pubkey::from_str(require_env("SOL_MINT").trim()).expect("Invalid SOL_MINT");
        assert_ne!(
            sol_mint, usdc_mint,
            "SOL_MINT and USDC_MINT_ADDRESS must differ"
        );
        assert_ne!(
            sol_mint, usdt_mint,
            "SOL_MINT and USDT_MINT_ADDRESS must differ"
        );
        assert_ne!(
            sol_mint, usdg_mint,
            "SOL_MINT and USDG_MINT_ADDRESS must differ"
        );
        let stable_coins = vec![
            StableCoinMint {
                code: "USDC".into(),
                mint: usdc_mint,
            },
            StableCoinMint {
                code: "USDT".into(),
                mint: usdt_mint,
            },
            StableCoinMint {
                code: "USDG".into(),
                mint: usdg_mint,
            },
        ];

        let wallet_master_seed = require_env("WALLET_MASTER_SEED");
        assert_wallet_master_seed(&wallet_master_seed);
        let wallet_encryption_key = require_env("WALLET_ENCRYPTION_KEY");
        assert_wallet_encryption_key(&wallet_encryption_key);

        let solana_rpc_url = require_env("SOLANA_RPC_URL");
        assert_public_https_url("SOLANA_RPC_URL", &solana_rpc_url);
        let solana_explorer_base = require_env("SOLANA_EXPLORER_BASE");
        assert_public_https_url("SOLANA_EXPLORER_BASE", &solana_explorer_base);

        let exchange_rate_api_url = require_env("EXCHANGE_RATE_API_URL");
        assert_public_https_url("EXCHANGE_RATE_API_URL", &exchange_rate_api_url);

        let airbills_base_url = require_env("AIRBILLS_BASE_URL")
            .trim()
            .trim_end_matches('/')
            .to_string();
        assert_public_https_url("AIRBILLS_BASE_URL", &airbills_base_url);
        let airbills_api_key = require_env("AIRBILLS_API_KEY");

        let jupiter_api_key = require_env_nonempty_trim("JUPITER_API_KEY");
        let jupiter_swap_base_url_raw = require_env_nonempty_trim("JUPITER_SWAP_BASE_URL");
        let jupiter_swap_base_url = jupiter_swap_base_url_raw.trim_end_matches('/').to_string();
        assert_public_https_url("JUPITER_SWAP_BASE_URL", &jupiter_swap_base_url);
        let jupiter_referral_account = require_env_nonempty_trim("JUPITER_REFERRAL_ACCOUNT");
        let _ = Pubkey::from_str(&jupiter_referral_account).unwrap_or_else(|e| {
            panic!("JUPITER_REFERRAL_ACCOUNT must be a valid Solana pubkey: {e:?}")
        });
        let jupiter_referral_fee_bps: u32 = require_parse("JUPITER_REFERRAL_FEE_BPS");

        let jupiter_lend_base_url_raw = require_env_nonempty_trim("JUPITER_LEND_BASE_URL");
        let jupiter_lend_base_url = jupiter_lend_base_url_raw
            .trim()
            .trim_end_matches('/')
            .to_string();
        assert_public_https_url("JUPITER_LEND_BASE_URL", &jupiter_lend_base_url);

        let payce_earn_fee_bps: u32 = std::env::var("PAYCE_EARN_FEE_BPS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(50);

        let paj_api_key = std::env::var("PAJ_API_KEY").unwrap_or_default();
        let paj_base_url_raw = std::env::var("PAJ_BASE_URL").unwrap_or_default();
        let paj_base_url = paj_base_url_raw.trim().trim_end_matches('/').to_string();
        if !paj_api_key.trim().is_empty() && !paj_base_url.is_empty() {
            assert_public_https_url("PAJ_BASE_URL", &paj_base_url);
        }

        let payce_public_base_url_raw = std::env::var("PAYCE_PUBLIC_BASE_URL").unwrap_or_default();
        let payce_public_base_url = payce_public_base_url_raw
            .trim()
            .trim_end_matches('/')
            .to_string();
        if !payce_public_base_url.is_empty() {
            assert_public_https_url("PAYCE_PUBLIC_BASE_URL", &payce_public_base_url);
        }

        let paj_webhook_secret = std::env::var("PAJ_WEBHOOK_SECRET").unwrap_or_default();
        let payce_internal_api_key = std::env::var("PAYCE_INTERNAL_API_KEY").unwrap_or_default();
        let resend_api_key = std::env::var("RESEND_API_KEY").unwrap_or_default();
        let resend_from = std::env::var("RESEND_FROM").unwrap_or_default();
        if !resend_api_key.trim().is_empty() && resend_from.trim().is_empty() {
            panic!("RESEND_FROM is required when RESEND_API_KEY is set");
        }

        let paj_ramp_business_usdc_fee = std::env::var("PAJ_RAMP_BUSINESS_USDC_FEE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| *v >= 0.0 && v.is_finite())
            .unwrap_or(1.0);

        let database_pool_max_size = std::env::var("DATABASE_POOL_MAX_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|v| *v > 0 && *v <= 1024)
            .unwrap_or(32);

        let node_env_for_check = require_env("NODE_ENV");
        if node_env_for_check == "production" && paj_webhook_secret.trim().is_empty() {
            panic!(
                "PAJ_WEBHOOK_SECRET is required in production (NODE_ENV=production). \
                 Set it to a long random value and configure PAJ to call /api/paj/webhooks/<id>?k=<secret>."
            );
        }

        let http = reqwest::Client::builder()
            .user_agent(concat!("payce-ng/", env!("CARGO_PKG_VERSION")))
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(32)
            .tcp_keepalive(Duration::from_secs(60))
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("failed to build shared reqwest::Client");

        Self {
            port: require_parse("PORT"),
            host: require_env("HOST"),
            node_env: require_env("NODE_ENV"),
            database_url: require_env("DATABASE_URL"),
            database_ssl: require_env("DATABASE_SSL"),
            redis_url: require_env("REDIS_URL"),
            solana_rpc_url,
            solana_network: require_env("SOLANA_NETWORK"),
            solana_explorer_base,
            usdc_mint,
            sol_mint,
            stable_coins,
            wallet_master_seed,
            wallet_encryption_key,
            at_api_key: require_env("AT_API_KEY"),
            at_username,
            at_sender_id: require_env("AT_SENDER_ID"),
            at_shortcode: shortcode_from_env(),
            at_messaging_url,
            at_callback_allowed_ips,
            at_callback_url_key,
            trust_proxy_xff,
            exchange_rate_api_url,
            exchange_rate_cache_ttl_secs: require_parse("EXCHANGE_RATE_CACHE_TTL_SECS"),
            exchange_rate_fallback_ngn: require_parse("EXCHANGE_RATE_FALLBACK_NGN"),
            exchange_rate_quote_currency: require_env("EXCHANGE_RATE_QUOTE_CURRENCY"),
            fee_payer: Arc::new(fee_payer),
            gas_fee_usdc,
            max_pin_attempts: require_parse("MAX_PIN_ATTEMPTS"),
            lockout_minutes: require_parse("PIN_LOCKOUT_MINUTES"),
            tier1_daily_ngn: require_parse("TIER1_DAILY_LIMIT_NGN"),
            tier2_daily_ngn: require_parse("TIER2_DAILY_LIMIT_NGN"),
            rate_limit_window_seconds: require_parse("RATE_LIMIT_WINDOW_SECONDS"),
            rate_limit_max_per_phone: require_parse("RATE_LIMIT_MAX_PER_PHONE"),
            rate_limit_max_per_ip: require_parse("RATE_LIMIT_MAX_PER_IP"),
            airbills_base_url,
            airbills_api_key,
            jupiter_api_key,
            jupiter_swap_base_url,
            jupiter_lend_base_url,
            jupiter_referral_account,
            jupiter_referral_fee_bps,
            payce_earn_fee_bps,
            paj_api_key,
            paj_base_url,
            payce_public_base_url,
            paj_webhook_secret,
            payce_internal_api_key,
            resend_api_key,
            resend_from,
            paj_ramp_business_usdc_fee,
            database_pool_max_size,
            http,
        }
    }

    pub fn is_production(&self) -> bool {
        self.node_env == "production"
    }

    pub fn paj_webhook_order_url(&self, order_id: &uuid::Uuid) -> Option<String> {
        let base = self.payce_public_base_url.trim().trim_end_matches('/');
        if base.is_empty() {
            return None;
        }
        let mut out = format!("{base}/api/paj/webhooks/{order_id}");
        let k = self.paj_webhook_secret.trim();
        if !k.is_empty() {
            out.push_str("?k=");
            out.push_str(k);
        }
        Some(out)
    }

    pub fn payce_ramp_api_enabled(&self) -> bool {
        !self.payce_internal_api_key.trim().is_empty()
    }

    pub fn ussd_stable_token_menu(&self) -> String {
        self.stable_coins
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, s.code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn stable_choice_index(&self, digit: &str) -> Option<usize> {
        let n: usize = digit.parse().ok()?;
        let max = self.stable_coins.len();
        if n >= 1 && n <= max {
            Some(n - 1)
        } else {
            None
        }
    }

    pub fn ussd_stable_pick_invalid_con(&self, header_line: Option<String>) -> String {
        let prefix = header_line
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| format!("{}\n", s))
            .unwrap_or_default();
        format!(
            "CON {prefix}Invalid choice. Pick token 1–{}:\n{}\nThen enter amount in Naira.",
            self.stable_coins.len(),
            self.ussd_stable_token_menu()
        )
    }
}

fn require_env_nonempty_trim(key: &'static str) -> String {
    let s = require_env(key);
    let t = s.trim().to_string();
    assert!(
        !t.is_empty(),
        "{key} cannot be empty after trim (set in the process environment or .env)"
    );
    t
}

fn require_env(key: &'static str) -> String {
    std::env::var(key)
        .unwrap_or_else(|_| panic!("{key} is required (set in the process environment or .env)"))
}

fn require_parse<T: std::str::FromStr>(key: &'static str) -> T
where
    <T as std::str::FromStr>::Err: std::fmt::Debug,
{
    let s = require_env(key);
    s.parse()
        .unwrap_or_else(|e| panic!("{key} must be a valid value: {e:?}"))
}

fn shortcode_from_env() -> Option<String> {
    let s = require_env("AT_SHORTCODE");
    let t = s.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

fn parse_csv_nonempty(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

# Payce Standard

Open-standard USSD payments infrastructure on Solana.

Built in Rust on top of Actix Web, Postgres, Redis, and Solana SDK.

Payce Standard is the USSD callback server behind [Payce](https://payce.xyz). 

It bridges Africa's Talking USSD sessions to Solana stablecoin transfers, Jupiter swap & lend, PAJ on/off-ramps, merchant flows, and utility bill purchases.

> **Status:** production-grade in design; reference implementation. Run on devnet first, audit before mainnet.

---

## Features

- **USSD callback handler** for Africa's Talking, with redacted logging and PIN-gated flows
- **Custodial wallets** derived per phone via HKDF-SHA256 from a master seed; encrypted at rest with AES-256-GCM
- **Stablecoin transfers** (USDC / USDT / USDG) with merchant code routing and KYC tier daily limits
- **Jupiter Swap v2** integration (12 stable ↔ stable / SOL routes)
- **Jupiter Lend Earn** integration with on-chain APY display normalization
- **PAJ ramps** for fiat ⇄ crypto with HMAC-secured webhooks and atomic state writes
- **Utility bills** (airtime, data, electricity, cable TV, betting) via Airbills
- **SMS receipts** + optional email receipts (Resend)
- **Unified activity ledger** (`user_activity` table) for analytics — Metabase-friendly schema
- **Background analytics writes** (Tokio spawn) so USSD response latency is unaffected by Postgres

## Architecture

```
                Africa's Talking USSD callback (HTTPS POST x-www-form-urlencoded)
                                       │
                              ┌────────▼────────┐
                              │   actix-web     │
                              │  /ussd/callback │
                              └────────┬────────┘
                                       │
                ┌──────────────────────┼─────────────────────┐
                │                      │                     │
        Phone IP rate-limit    User lookup           USSD state machine
        (Redis incr+expire)    (Postgres deadpool)   (services/ussd_menu/*)
                                       │
                ┌──────────────────────┼─────────────────────┐
                │                      │                     │
        Solana RPC               Postgres               External APIs
        (sendTransaction,        (transactions,         Jupiter, PAJ, Airbills,
         getBalance,              users, paj_orders,    Resend, Africa's Talking
         getTokenAccountBalance)  user_activity)        — all share one reqwest::Client
                                       │
                              fire-and-forget tokio::spawn
                              for `user_activity` ledger writes
                              (insert/upsert by ref_id)
```

Source layout:

```
src/
├── lib.rs                  # actix-web bootstrapping, routes
├── main.rs                 # binary entrypoint (loads .env, env_logger)
├── config/                 # AppConfig: validates env, owns shared reqwest::Client
├── db.rs / db_pool.rs      # Postgres pool + startup migrations
├── middleware/             # auth, rate_limit, security headers
├── routes/                 # HTTP handlers (USSD, PAJ, merchant, health)
├── services/
│   ├── ussd_menu/          # state machine for each USSD branch
│   ├── transfer.rs         # SPL transfers, daily limits
│   ├── jupiter_swap.rs     # Jupiter Swap v2
│   ├── jupiter_earn.rs     # Jupiter Lend Earn
│   ├── paj_*.rs            # PAJ ramp client + webhook + state machine
│   ├── airbills.rs         # utility bills
│   ├── user_activity.rs    # unified activity ledger (race-safe upsert)
│   └── ...
└── utils/                  # crypto, phone normalization, validators

migrations/                 # SQL migrations applied on startup
```

## Quick start (local)

Requirements: Rust ≥ 1.75, Docker (for Postgres + Redis), or a local Postgres 16 + Redis 7.

```bash
git clone https://github.com/Axio-Lab/payce-standard.git
cd payce-standard
cp .env.example .env
# Fill in .env — at minimum: WALLET_MASTER_SEED, WALLET_ENCRYPTION_KEY,
# FEE_PAYER_PRIVATE_KEY, USDC_MINT_ADDRESS, USDT_MINT_ADDRESS, USDG_MINT_ADDRESS,
# JUPITER_API_KEY, AT_API_KEY, AIRBILLS_API_KEY.
docker compose up -d           # Postgres on :5433, Redis on :6379
cargo run                      # listens on :3000
curl -s http://localhost:3000/health
```

Test a USSD session locally:

```bash
curl -s -X POST http://localhost:3000/ussd/callback \
  -d 'sessionId=local-1' \
  -d 'phoneNumber=+2347000000001' \
  -d 'serviceCode=*384#' \
  -d 'text='
```

## Configuration

Every variable is read from the process environment (or `.env` for local dev). See [`.env.example`](.env.example) for the full list with inline documentation.

A few that warrant special attention:

- `WALLET_MASTER_SEED` — HKDF input. Losing it bricks every wallet.
- `WALLET_ENCRYPTION_KEY` — 64 hex chars (32 bytes). AES-256-GCM key for encrypted keypair storage.
- `FEE_PAYER_PRIVATE_KEY` — base58 keypair. Funds gas; never share.
- `PAJ_WEBHOOK_SECRET` — required in `NODE_ENV=production`; appended as `?k=<secret>` and compared in constant time.
- `TRUSTED_PROXY_IPS` — only set if you front the app with a known proxy. Setting it lets that proxy's `X-Forwarded-For` decide the client IP for the callback allowlist.
- `DATABASE_POOL_MAX_SIZE` — default 32. Bump for higher-concurrency deployments.

## Database migrations

Migrations live in [`migrations/`](migrations) and are applied on startup in filename order. The runner records applied files in `schema_migrations` and is idempotent against pre-existing schemas (initial migration auto-marks if `users` already exists).

To add a new migration: drop a `NNN_short_name.sql` file in `migrations/`, restart the server.

## Security

- All long-lived secrets (AT API key, internal API key, PAJ webhook secret) are compared in constant time (`subtle` crate).
- The Africa's Talking IP allowlist is enforced against the **direct peer IP** by default. `X-Forwarded-For` is honored only when the peer is in `TRUSTED_PROXY_IPS`.
- Production responses include HSTS (`max-age=63072000; includeSubDomains; preload`), `X-Content-Type-Options`, `X-Frame-Options: DENY`, and a strict `Permissions-Policy`.
- Wallet keypairs are AES-256-GCM encrypted with a 12-byte random nonce per record.
- USSD logs redact 4-digit PIN segments and long opaque inputs.

To report a vulnerability please read [SECURITY.md](SECURITY.md).

## Testing

```bash
cargo test           # unit + formatting tests
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

CI runs all three on every push (`.github/workflows/ci.yml`).

## Deployment

A minimal `Dockerfile` is provided. Build:

```bash
docker build -t payce-standard:dev .
docker run --rm -p 3000:3000 --env-file .env payce-standard:dev
```

For production, terminate TLS at a reverse proxy (Caddy, nginx, ALB) and set `TRUSTED_PROXY_IPS` to that proxy's IP. Always run with `DATABASE_SSL=require` against managed Postgres.


## License

[MIT](LICENSE).

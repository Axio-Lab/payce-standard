# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Payce Standard, please **do not open a public GitHub issue**. Instead, email the maintainers at **security@axiolabs.dev** (replace with your real address before publishing).

Please include:

- A clear description of the issue and its impact
- Steps to reproduce (proof-of-concept code if available)
- The commit hash / version you tested against
- Any suggested mitigation

We aim to acknowledge reports within **72 hours** and ship a fix or mitigation within **14 days** for high-severity issues.

## Supported Versions

The `main` branch is the only actively maintained version while the project is pre-1.0. Once we tag stable releases this section will list each supported minor version's end-of-life date.

## Hardening checklist (operators)

If you run Payce Standard in production, please ensure all of the following:

- `NODE_ENV=production` (enables HSTS, AT IP allowlist, fails closed if `PAJ_WEBHOOK_SECRET` is unset)
- `DATABASE_SSL=require` against a managed Postgres
- `WALLET_MASTER_SEED` and `WALLET_ENCRYPTION_KEY` stored in a real secret manager (KMS, Vault, Doppler, Railway secrets) — never in `.env` on the host
- `FEE_PAYER_PRIVATE_KEY` rotated periodically; balance kept low (only enough for ~1 week of gas)
- TLS termination at a reverse proxy with `TRUSTED_PROXY_IPS` set to that proxy's IP, so `X-Forwarded-For` cannot be spoofed
- Postgres backups enabled with point-in-time recovery
- Redis used only for rate-limit + session state (no PII stored long-term)
- `RATE_LIMIT_MAX_PER_PHONE` and `RATE_LIMIT_MAX_PER_IP` tuned for your traffic
- Africa's Talking shortcode locked to your account; webhook URL using HTTPS only

## What's in scope

- Authentication / authorization bypass
- Webhook spoofing or replay
- Wallet keypair leakage or weakening of HKDF / AES-GCM
- SQL injection, command injection, SSRF
- Denial of service via the USSD callback or webhook routes
- Rate-limit / IP allowlist bypass

## What's out of scope

- Vulnerabilities in third-party services (Africa's Talking, Jupiter, PAJ, Airbills, Resend) — please report those upstream
- Issues only reproducible against a misconfigured deployment (e.g. `NODE_ENV` not set to `production`)
- Self-XSS / theoretical attacks without a concrete impact path

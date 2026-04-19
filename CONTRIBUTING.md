# Contributing to Payce Standard

Thanks for your interest! This project is the reference USSD payments server for Solana — we welcome focused, well-scoped pull requests.

## Before you open a PR

1. **Discuss large changes first.** Open an issue describing the problem and proposed approach before sinking time into a big refactor.
2. **One concern per PR.** Smaller PRs ship faster and are easier to review.
3. **Don't break the USSD response budget.** USSD sessions have a hard ~10s carrier deadline. Anything on the request path must be non-blocking or fast (use `tokio::spawn` for analytics writes — see `services/user_activity.rs` for the pattern).

## Local setup

```bash
cp .env.example .env       # fill in required vars (see README)
docker compose up -d       # Postgres + Redis
cargo run
```

## Quality bar

Every PR must pass:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo check --release
```

CI runs the same checks (`.github/workflows/ci.yml`).

### Style

- Follow the existing module layout (`services/`, `routes/`, `middleware/`, `utils/`, `config/`).
- Prefer small free functions over large impl blocks.
- Don't add comments that just narrate what the code does. Reserve comments for non-obvious intent, trade-offs, and constraints.
- Log at `info` for one-line state changes, `warn` for recoverable problems, `error` for things an operator must see. Never log full PII or secrets — see `mask_phone`, `redact_text`, etc.

### Database changes

- Add a new migration as `migrations/NNN_short_name.sql` (NNN = next number).
- Migrations must be idempotent where possible (use `IF NOT EXISTS`).
- Don't `DROP` columns in the same migration that the app is reading from — split into two deploys.

### Security-sensitive changes

If your change touches authentication, signing, encryption, webhook validation, or the rate limiter, please add a brief threat-model note in the PR description: what attacker capability does this enable / mitigate?

## Reporting bugs

- Public bugs → GitHub Issues
- Security vulnerabilities → see [SECURITY.md](SECURITY.md)

## Code of conduct

Be kind. We follow the [Contributor Covenant v2.1](https://www.contributor-covenant.org/version/2/1/code_of_conduct/) — harassment, personal attacks, and discrimination will get you banned from the project.

## License

By contributing you agree to license your work under the [MIT License](LICENSE).

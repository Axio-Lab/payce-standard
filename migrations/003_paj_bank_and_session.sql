ALTER TABLE users
    ADD COLUMN IF NOT EXISTS email VARCHAR(320),
    ADD COLUMN IF NOT EXISTS email_verified_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS paj_session_token TEXT,
    ADD COLUMN IF NOT EXISTS paj_session_expires_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_users_email_lower
    ON users (lower(btrim(email)))
    WHERE email IS NOT NULL AND btrim(email) <> '';

CREATE TABLE IF NOT EXISTS user_paj_bank_accounts (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    paj_saved_account_id TEXT NOT NULL,
    bank_code VARCHAR(32),
    bank_name VARCHAR(255),
    account_number VARCHAR(32) NOT NULL,
    account_name VARCHAR(255),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, paj_saved_account_id)
);

CREATE INDEX IF NOT EXISTS idx_user_paj_bank_accounts_user
    ON user_paj_bank_accounts (user_id);

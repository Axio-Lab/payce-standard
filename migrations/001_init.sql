CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TYPE user_status AS ENUM ('PENDING', 'ACTIVE', 'LOCKED', 'SUSPENDED');
CREATE TYPE kyc_tier AS ENUM ('TIER_0', 'TIER_1', 'TIER_2');
CREATE TYPE merchant_status AS ENUM ('PENDING', 'ACTIVE', 'SUSPENDED');
CREATE TYPE transaction_type AS ENUM ('P2P', 'MERCHANT_PAYMENT', 'CASH_IN', 'CASH_OUT');
CREATE TYPE transaction_status AS ENUM ('PENDING', 'CONFIRMED', 'FAILED');

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    phone_number VARCHAR(20) NOT NULL UNIQUE,
    pin_hash VARCHAR(255),
    solana_pubkey VARCHAR(64) UNIQUE,
    encrypted_keypair TEXT,
    secondary_solana_pubkey VARCHAR(64) UNIQUE,
    secondary_encrypted_keypair TEXT,
    kyc_tier kyc_tier NOT NULL DEFAULT 'TIER_0',
    status user_status NOT NULL DEFAULT 'PENDING',
    failed_pin_attempts INTEGER NOT NULL DEFAULT 0,
    locked_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_users_phone ON users(phone_number);

CREATE TABLE merchants (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    merchant_code VARCHAR(10) NOT NULL UNIQUE,
    business_name VARCHAR(255) NOT NULL,
    category VARCHAR(100) NOT NULL DEFAULT 'general',
    status merchant_status NOT NULL DEFAULT 'PENDING',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_merchants_code ON merchants(merchant_code);

CREATE TABLE transactions (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    sender_id UUID NOT NULL REFERENCES users(id),
    recipient_id UUID REFERENCES users(id),
    merchant_id UUID REFERENCES merchants(id),
    amount_usdc BIGINT NOT NULL,
    amount_ngn DOUBLE PRECISION NOT NULL,
    exchange_rate DOUBLE PRECISION NOT NULL,
    type transaction_type NOT NULL,
    status transaction_status NOT NULL DEFAULT 'PENDING',
    tx_signature VARCHAR(128),
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_transactions_sender ON transactions(sender_id);
CREATE INDEX idx_transactions_recipient ON transactions(recipient_id);
CREATE INDEX idx_transactions_merchant ON transactions(merchant_id);
CREATE INDEX idx_transactions_created ON transactions(created_at);

CREATE TABLE favorite_contacts (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    alias VARCHAR(100) NOT NULL,
    address VARCHAR(64) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, alias)
);

CREATE INDEX idx_favorites_user ON favorite_contacts(user_id);

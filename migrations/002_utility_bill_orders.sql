CREATE TYPE utility_bill_status AS ENUM (
    'PENDING',
    'ONCHAIN_SUBMITTED',
    'PROCESSING',
    'COMPLETED',
    'FAILED'
);

CREATE TABLE utility_bill_orders (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    airbills_id TEXT NOT NULL,
    product_code TEXT NOT NULL,
    status utility_bill_status NOT NULL DEFAULT 'PENDING',
    chain_tx_signature VARCHAR(128),
    amount_ngn DOUBLE PRECISION NOT NULL,
    amount_usdc DOUBLE PRECISION,
    exchange_rate DOUBLE PRECISION,
    error_message TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(airbills_id)
);

CREATE INDEX idx_utility_bill_orders_user ON utility_bill_orders(user_id);
CREATE INDEX idx_utility_bill_orders_created ON utility_bill_orders(created_at);
CREATE INDEX idx_utility_bill_orders_status ON utility_bill_orders(status);

-- Unified activity ledger for analytics (e.g. Metabase) across transfers, DeFi, bills, PAJ.

CREATE TABLE user_activity (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4 (),
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'CONFIRMED',
    tx_signature VARCHAR (256),
    amount_raw BIGINT,
    denom_mint TEXT,
    amount_ngn DOUBLE PRECISION,
    exchange_rate DOUBLE PRECISION,
    counterparty_user_id UUID REFERENCES users (id),
    ref_id TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_user_activity_user_created ON user_activity (user_id, created_at DESC);

CREATE INDEX idx_user_activity_event_type ON user_activity (event_type);

CREATE INDEX idx_user_activity_ref_id ON user_activity (ref_id)
WHERE
    ref_id IS NOT NULL
    AND btrim(ref_id) <> '';

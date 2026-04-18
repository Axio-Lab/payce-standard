CREATE TABLE IF NOT EXISTS paj_orders (
    id UUID PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    direction TEXT NOT NULL CHECK (direction IN ('onramp', 'offramp')),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (
        status IN ('pending', 'success', 'failed', 'unknown')
    ),
    paj_order_id TEXT,
    mint TEXT,
    chain TEXT,
    currency TEXT,
    request_json JSONB,
    response_json JSONB,
    last_webhook_payload JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_paj_orders_user_created
    ON paj_orders (user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_paj_orders_paj_order_id
    ON paj_orders (paj_order_id)
    WHERE paj_order_id IS NOT NULL AND btrim(paj_order_id) <> '';

CREATE TABLE IF NOT EXISTS paj_order_events (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4 (),
    order_id UUID NOT NULL REFERENCES paj_orders (id) ON DELETE CASCADE,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_paj_order_events_order
    ON paj_order_events (order_id, created_at DESC);

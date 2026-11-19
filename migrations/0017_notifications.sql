-- 0017: Notification buffer

CREATE TABLE IF NOT EXISTS notification_buffer (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    -- Target
    identity_id     UUID NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    platform        platform NOT NULL,
    -- Content
    priority        TEXT NOT NULL DEFAULT 'normal',
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    metadata        JSONB NOT NULL DEFAULT '{}',
    -- Status
    delivered       BOOLEAN NOT NULL DEFAULT false,
    delivered_at    TIMESTAMPTZ,
    -- Timestamps
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deliver_after   TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_notification_buffer_undelivered
    ON notification_buffer (identity_id, deliver_after)
    WHERE delivered = false;

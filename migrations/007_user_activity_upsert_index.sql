

CREATE UNIQUE INDEX IF NOT EXISTS uniq_user_activity_ref_event ON user_activity (ref_id, event_type)
WHERE
    ref_id IS NOT NULL
    AND btrim(ref_id) <> '';

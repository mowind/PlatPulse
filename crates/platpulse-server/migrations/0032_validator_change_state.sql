ALTER TABLE current_validator_insights
    ADD COLUMN change_state TEXT NOT NULL DEFAULT 'normal'
    CHECK (change_state IN ('normal', 'ranking_changed', 'counter_reset'));

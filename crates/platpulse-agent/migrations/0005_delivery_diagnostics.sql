-- Local bounded delivery diagnostics. Immutable reports remain in reports.
CREATE TABLE delivery_diagnostics (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    last_error TEXT,
    last_error_at TEXT
);

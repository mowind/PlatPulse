-- Coarse, non-sensitive client hint for human Sessions (design §12.3):
-- the Server never stores a full long-lived User-Agent or a raw IP; it
-- records only a coarse browser/platform family derived at login, so the
-- Session review surface can distinguish active sessions without retaining
-- sensitive client details.
ALTER TABLE sessions ADD COLUMN client_hint TEXT NOT NULL DEFAULT 'Unknown';

-- Preserve the receipt time of the last successful component value separately
-- from the receipt time of the latest collection state/error.
ALTER TABLE component_status ADD COLUMN value_received_at TEXT;

-- Existing successful values can be dated from their current receipt. Error
-- states cannot reliably recover the prior value receipt, so leave those NULL
-- rather than presenting the error receipt as fresh value evidence.
UPDATE component_status
   SET value_received_at = received_at
 WHERE state = 'ok' AND value_revision > 0;

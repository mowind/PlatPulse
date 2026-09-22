-- The Inventory Declaration Record (issue #181).
--
-- The record is the last Node Inventory the Server *effectively accepted* from
-- this Agent: its revision and content hash, taken from the Report Receipt
-- that accepted it. It exists so an Agent can refuse to declare changed
-- Inventory content under an unchanged `inventory_revision` instead of
-- re-sending a report the Server will refuse forever.
--
-- A separate singleton table rather than columns on `agent_state`: the record
-- is a fact about the accepted declaration, `agent_state.inventory_revision`
-- is the last *declared* revision (written at persist time), and no row here
-- must stay distinguishable from "revision 0". The Agent Store is the only
-- writer, inside the receipt-application transaction.
CREATE TABLE inventory_declaration (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    sha256 TEXT NOT NULL,
    report_id TEXT NOT NULL,
    adopted_at TEXT NOT NULL
);

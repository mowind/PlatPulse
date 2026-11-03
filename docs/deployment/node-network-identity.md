Part of the [PlatPulse Server deployment guide](../deployment.md): Node Network Identity at onboarding and after a Node build upgrade.

## Node Network Identity at onboarding and after a Node build upgrade

A Network's identity is its Registry tuple: genesis hash, chain ID, P2P network ID, and address HRP. The Agent stamps every Block Summary with the identity tuple its own Node reports, and the Server compares that stamp with the Registry row for the Node's Network key. A sample whose stamp differs is terminally rejected with `network_identity_mismatch` ("Block network identity does not match the registered Network"); the rejected interval is a History Gap that is never merged, re-labelled, or backfilled.

Confirm a Node's own identity before adding it to a Network, and again after any Node build upgrade:

- `eth_chainId` must answer the chain ID registered for that Network — never a value borrowed from another chain. On PlatON mainnet that is 210425 (`0x335f9`), and a pre-PIP-7 build answers the legacy `100` instead; PlatON's own testnet is a different chain (210429, legacy `104`). Take the expected values from this Network's Registry row, not from a generalised example.
- `admin_nodeInfo` must report the same PIP-7 chain ID, while the legacy `chainId` field may still report the pre-PIP-7 value.
- `platon version` must be at or after commit `45c3e086d`, the PIP-7 chain-ID fix. Builds without it (for example `v1.5.1-unstable-34e50429-20251023`) still answer `eth_chainId` with `100`.

Symptoms of a mismatching Node: Admin → Home raises the critical Network Identity Mismatch attention item (`node_identity_mismatch`); Admin → Networks and the Node page show the Node as Mismatched with the contradicting field names and the observed tuple beside the Registry tuple; every Block Summary from that Node is rejected, so the Blocks and Txs of its heights stay unknown. The mismatch is repaired on the Node side, never in the Registry:

1. Correct the Node: upgrade its binary to a build containing the chain-ID fix, or correct its network configuration, and restart it.
2. Confirm the Node now answers the Registry tuple — repeat the checks above — and let the Agent's collection run at least once afterwards (`collection_interval_seconds`), so a Report carrying the new identity is persisted, before you touch the Agent. A restart taken while the Agent still holds the pre-upgrade identity is not harmless: its Block workers stamp samples from the identity in the most recent persisted Report (`load_last_report` in `crates/platpulse-agent/src/collector.rs`), those workers start before the Report collection loop (`crates/platpulse-agent/src/cli.rs`), and a graceful stop persists last-good observations without probing the Node (`crates/platpulse-agent/src/shutdown.rs`). Without a successful post-upgrade collection the restarted Agent can keep the stale stamp for a whole subscription and extend the permanent gap.
3. Restart that Host's Agent (`sudo systemctl restart platpulse-agent`, or the matching `systemctl --user` unit), then verify the outcome instead of assuming it: the Node must leave the Mismatched state, the `node_identity_mismatch` alert must clear, and its new heights must stop arriving as rejected samples. If samples are still rejected, wait one `collection_interval_seconds` so a fresh Report is persisted, then restart the Agent once more.
4. Do not edit the Registry tuple to match a Node (Admin → Networks → Edit tuple, or `PUT /api/admin/v1/networks/{network_key}`). The observed tuple is the Node's own claim; matching it would contradict every other Node on the same Network, and it reclaims no already-rejected Block History.

### When the observation matches but the Block history is still rejected

A second shape of the same failure is the one that reads as a contradiction (issue #235): the Node's observed identity **matches** the Registry, Admin → Home shows the `node_identity_mismatch` attention item as resolved, and yet every Block Summary from that Node is still terminally rejected, so its Blocks and Txs stay Unknown and new History Gaps keep arriving. Nothing is wrong with the Node or the Registry — the **Agent's Block Identity Stamp** is stale.

The Agent takes that stamp from the most recent persisted Report when its Block Worker subscribes to the Node (`load_last_report` in `crates/platpulse-agent/src/collector.rs`) and reuses it for the whole subscription. A Node rebuild, reconfiguration, or binary swap does not necessarily fail the Agent's own RPC connection, so that subscription — and with it the stamp — can outlive the identity change; the Server, which only ever compares the stamp it is given, keeps refusing samples that carry the old tuple. The stamp is refreshed when the Agent re-subscribes or restarts, never by anything the Server or the Registry does.

Where the Server says so: Admin → the Node's detail page → Network identity renders a **History rejected** diagnosis that states the refusal in the Server's own words — every differing field with its stamped and registered value, which side was found stale (the Agent's stamp, or a genuine cross-Network conflict because the current observation contradicts the Registry as well), the refused height range, and the most recent refusal time. A stamp that carries no address HRP at all reads as `not reported`, never as a value the Registry might hold. That diagnosis is built only from the Server's own refusal rows (`block_history_gaps` where `kind = 'server_rejected'` and `authored_by = 'server'`); a History Gap an Agent reports about itself is stored as the Agent's own evidence and can never present itself as the Server's diagnosis. The same reason text is stored on the History Gap row, so it survives in a snapshot and in the audit trail, and it is what the Node's Receipt reports as the refusal, so the two surfaces always name the same cause. Each sample is judged on the stamp it was collected with, so a Report spooled across a subscription boundary reports each sample's own differing fields. A diagnosis that names the current observation as contradicting too is not a stale stamp: repair it with the mismatch steps above.

1. Restart that Host's Agent (`sudo systemctl restart platpulse-agent`, or the matching `systemctl --user` unit). That is the only repair: the Node is already correct.
2. Verify the outcome instead of assuming it: the Node's new heights must stop arriving as rejected samples and it must start producing accepted Block Summaries again, so its Blocks and Txs stop being Unknown. The History rejected diagnosis clears itself once the Server accepts a Block Summary again — it names the newest refusal only while that refusal is later than the newest accepted summary. That acceptance evidence is kept on the Node's durable history state (`block_history_state.last_accepted_summary_at`), so the diagnosis cannot come back for a Node that already recovered once raw Block Summaries age out of retention.
3. Accept the loss: the already-refused interval stays a permanent History Gap. Those heights are never merged, re-labelled, or backfilled, and the Registry tuple is still never edited to match a stamped sample (step 4 above).

### When a Node answers millisecond timestamps

A third shape of "the Block history is rejected" never reaches the Server at all: the Agent cannot assemble a Report, exits non-zero, and its unit restarts it in a loop. The journal shows the same fatal line about a second after every start:

```text
Agent report collection failed: Collection("invalid persisted Agent identity: field block.block_timestamp_ms is outside its contract range")
```

The value that line names is a **millisecond** timestamp read as seconds (issue #243). PlatON native headers carry `header.Time` in milliseconds, and the `eth_*` aliases the Agent uses (`eth_getBlockByNumber`, `eth_subscribe`) divide it by 1000 only when the Node runs with `--http.ethcompatible`. A Node started without that flag hands the Agent a raw millisecond value such as `1792034712000`, which an Agent older than this fix multiplied by 1000 again and stored as `1792034712000000` — far beyond the AgentReport v1 contract limit of `4102444800000` ms (2100-01-01T00:00:00Z). Every Report assembled from that stored sample is refused, so collection can never succeed and the restart loop repeats. Current Agents refuse such a sample at the persistence boundary with an actionable message that names the limit and the flag, skip an already-stored one when loading samples, and sweep the Store once at startup, so the loop cannot start. The unit is read from the value itself: `100000000000` or more is already milliseconds (that magnitude read as seconds would be the year 5138), anything smaller is seconds.

Run every PlatON Node with `--http.ethcompatible`, and confirm the flag and the unit before you touch the Agent:

```bash
# The flag must be among the Node's own arguments.
ps -o args= -C platon | tr ' ' '\n' | grep -x -- '--http.ethcompatible'

# The timestamp must be seconds: 10 hex digits, for example 0x6ad04a70
# (1792035440), never a millisecond value such as 0x1a13d97a9c0 (1792034712000).
curl -s -X POST -H 'Content-Type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["latest",false]}' \
  http://127.0.0.1:6789 | head -c 400
```

Then repair in this order:

1. Restart the Node with `--http.ethcompatible` (or with the flag its Network's Nodes are expected to carry), and confirm the timestamp above is now seconds.
2. Restart that Host's Agent (`sudo systemctl restart platpulse-agent`, or the matching `systemctl --user` unit). On startup it sweeps its own Agent Store, removes every Block Summary whose chain timestamp is outside the contract, and declares what it removed as a History Gap. It logs one line per affected Node, followed by the number of ranges it declared:

```text
Purged 3 Block Summary sample(s) outside the AgentReport v1 contract (0195f2a1-0014-4014-8014-000000000014: 3), declared as 1 History Gap range(s). Check that every Node runs with --http.ethcompatible.
```

   The sweep is also the repair for a Store poisoned before the Agent learned to refuse such a sample, so a hand-written `DELETE` is only needed on an Agent build older than this fix. The sweep covers the Nodes this Agent declares in its Inventory: rows of a Node that is no longer declared are left for a later start, because a History Gap naming an undeclared Node is refused by the contract. Each Node's consecutive removed heights become one `unrecoverable_backfill` History Gap per contiguous range in its `history_gaps` (a run longer than the 1,000,000-height contract limit is split so the declaration itself stays inside the contract), reported to the Server so those heights are never presented as continuous history. **Nothing refills them automatically:** the Agent resumes recovery at the height recorded for that Node and never schedules a lower one again, and the Server discards a replay of heights below its own high-water mark. Treat the declared gap as the permanent record it is, and fetch those Blocks out of band from the Node while it still retains them if they matter.
3. Verify the outcome instead of assuming it: the Agent stays up (`systemctl status platpulse-agent`) and its next Reports are accepted instead of failing validation. Read-only inspection of the Agent Store (the configured `state_db`, packaged default `/var/lib/platpulse-agent/agent.db`) needs the Agent stopped, as the Store is written under WAL with exclusive locking:

```bash
STATE_DB=/var/lib/platpulse-agent/agent.db   # the Agent config key state_db, Agent stopped
sqlite3 "$STATE_DB" \
  "SELECT node_id, COUNT(*) AS samples, MIN(block_timestamp_ms), MAX(block_timestamp_ms)
     FROM block_summaries
    WHERE block_timestamp_ms > 4102444800000
    GROUP BY node_id;"
```

   A row that still shows up after a restart of the current Agent means the row was written after the sweep (check the Agent version), or the Agent is not reading the `state_db` you queried.

Read-only verification. The packaged units keep the Server database at `/var/lib/platpulse/platpulse.db` (see the configured `db_path`). Query a **consistent snapshot**, never the live file: either the artifact written by the packaged `platpulse-server backup` command in an [Offline Backup Window](agent-store-systemd-and-backups.md#offline-backups-no-packaged-automation) (the Server stopped, as that command requires), or a copy of `db_path` taken while the Server is stopped. The live database is written under WAL with exclusive locking, so opening it directly under the `immutable` URI bypasses that synchronisation — it can miss not-yet-checkpointed WAL content, and under concurrent writes it can even fail with `database disk image is malformed`. A live `immutable` read is a best-effort eyeball, never evidence:

```bash
# Packaged `backup` artifact, or a copy of db_path taken while the Server was stopped.
SNAPSHOT=/var/backups/platpulse/<backup-artifact>

sqlite3 "$SNAPSHOT" \
  "SELECT n.display_name, o.network_chain_id, o.network_p2p_network_id,
          o.network_address_hrp, o.network_genesis_hash
     FROM nodes n
     JOIN current_node_chain_observations o ON o.node_id = n.node_id
    WHERE n.network_key = 'REPLACE_WITH_NETWORK_KEY';"

sqlite3 "$SNAPSHOT" \
  "SELECT network_key, chain_id, p2p_network_id, address_hrp, genesis_hash FROM networks;"

# The refusals one Node's stamp produced, and the Server's own newest reason for them.
sqlite3 "$SNAPSHOT" \
  "SELECT kind, MIN(from_height) AS first_height, MAX(to_height) AS last_height,
          COUNT(*) AS gap_rows, MAX(created_at) AS latest
     FROM block_history_gaps
    WHERE node_id = 'REPLACE_WITH_NODE_ID'
    GROUP BY kind;"

sqlite3 "$SNAPSHOT" \
  "SELECT reason FROM block_history_gaps
    WHERE node_id = 'REPLACE_WITH_NODE_ID' AND kind = 'server_rejected'
      AND authored_by = 'server'
    ORDER BY created_at DESC, from_height DESC LIMIT 1;"
```

A Node's `display_name` is assigned when the Node is first accepted; a later Inventory declaration does not rename it, so rename the Node through the Admin Node metadata action (`PUT /api/admin/v1/nodes/{node_id}/metadata`).

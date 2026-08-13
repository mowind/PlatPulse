# platpulse-agent

The PlatPulse collector process that runs on a Host next to its PlatON Nodes
and reports each Node's observations independently.

## Node RPC transport (Alloy)

All Node RPC and block traffic goes through the **pinned custom Alloy fork**,
the same source ChainDash is validated against:

```toml
alloy = { git = "https://github.com/mowind/alloy",
          rev = "51d20673f7ce1dafe6f4aaad304347b0315dc526",
          features = ["provider-ws", "provider-ipc", "pubsub", "provider-debug-api"] }
```

- `provider-ws` covers both `ws://` and `wss://`; `provider-ipc` covers local
  `ipc://` sockets; `pubsub` provides `eth_subscribe("newHeads")`;
  `provider-debug-api` provides the PlatON `debug_consensusStatus` types.
- Alloy is confined to the transport layer: `src/rpc.rs` (capability/identity
  probe adapter) and `src/block.rs` (per-Node head subscription and block
  resolution). Collector, store, and report code only see the synchronous
  `RpcAdapter` / `BlockTransport` seams and the async
  `WebSocketBlockTransport` methods.

### RPC interface rules

- The Agent uses the **standard `eth_*` interfaces** (`eth_blockNumber`,
  `eth_getBlockByNumber`, `eth_getBlockByHash`, `eth_syncing`,
  `eth_chainId`, `eth_subscribe`) — never the `platon_*` namespace. PlatON's
  `eth_*` aliases return seconds timestamps, which the transport converts to
  Unix **milliseconds** for the wire contract (`block_timestamp_ms`), and
  `0x`-hex addresses; no bech32 parsing is done anywhere.
- Unwrapped PlatON methods go through Alloy raw requests with a **bounded
  local DTO**: the method must be in the transport allowlist, the decoded
  value is size-checked before deserialization, and every field is parsed
  with quantity/hex/char bounds. `admin_peers`, `admin_nodeInfo`,
  `web3_clientVersion`, and `rpc_modules` follow this pattern in `src/rpc.rs`.

### Adding a new raw method

1. Add the method name to the transport's `allowed_methods` (or the probe's
   call list) so unknown/admin/debug methods stay fail-closed.
2. Define a bounded `#[derive(Deserialize)]` DTO with `camelCase` field
   names and `Option`/bounded fields — never accept raw JSON into the report.
3. Call it via `provider.raw_request` (through `request_value` in
   `block.rs`, which applies the size bound), then validate quantities and
   lengths exactly once.

### Live-node integration checks

`tests/live_node.rs` exercises the real transport against a running PlatON
node. They are skipped unless the node URL is provided:

```text
PLATPULSE_TEST_RPC_URL=ws://127.0.0.1:6790 \
  cargo test -p platpulse-agent --test live_node
```

The checks cover the capability/identity probe, genesis/head resolution
(including the seconds→ms timestamp conversion), and the `newHeads`
subscription round-trip.

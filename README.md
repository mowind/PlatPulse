<div align="center">
  <img src="assets/platpulse-logo.svg" alt="PlatPulse logo" width="760">
  <p><strong>A Server–Agent–WebUI monitoring suite for PlatON nodes.</strong></p>
  <p>
    Real-time health metrics · Consensus status · Peer insights · Validator analytics · Alerts
  </p>
</div>

# PlatPulse

PlatPulse is an open-source monitoring suite designed to make PlatON node operations observable, actionable, and easy to scale.

It collects node and chain observations through lightweight Agents, centralizes them in a Server, and presents current health and operational insights through a WebUI.

> **Status:** Early architecture and workspace setup.

## Why PlatPulse

Operating blockchain infrastructure requires more than a single process health check. PlatPulse is designed to bring the operational picture together in one place:

- **Node health** — process, system, synchronization, and observation freshness
- **Consensus visibility** — consensus state and chain progress
- **Peer insights** — per-node peer connectivity and peer state
- **Validator analytics** — validator-oriented metrics and operational signals
- **Alerts** — actionable conditions with durable notification delivery

## Architecture

PlatPulse is a new, independent project with a greenfield Server–Agent–WebUI architecture.

```text
┌──────────────────┐      AgentReport v1      ┌──────────────────┐
│  PlatON node(s)  │ ◄─── platpulse-agent ─── │ platpulse-server │
└──────────────────┘                          └────────┬─────────┘
                                                       │
                                      SQLite projections│ REST API
                                                       ▼
                                               ┌────────────────┐
                                               │  platpulse-web │
                                               └────────────────┘
                                                       │
                                                       ▼
                                             Alerts / notifications
```

### Workspace components

| Component | Responsibility |
| --- | --- |
| `platpulse-core` | Shared domain types, protocol definitions, and versioned contracts |
| `platpulse-agent` | Runs near PlatON nodes and publishes health and chain observations |
| `platpulse-server` | Ingests reports, maintains current-state projections, exposes the API, and evaluates alerts |
| `platpulse-web` | Provides dashboards for node health, consensus, peers, validators, and alerts |
| `platpulse-tui` *(future)* | Optional terminal interface for operators who prefer a CLI workflow |

The Agent–Server protocol is centered on versioned `AgentReport` messages. The Server turns incoming observations into current-state projections while preserving revision and freshness semantics. Durable spooling on the Agent side and a notification outbox on the Server side are intended to keep temporary outages from becoming lost data or lost alerts.

## Roadmap

- [ ] Define the workspace and protocol versioning
- [ ] Implement `AgentReport` v1
- [ ] Build the first Agent → Server ingestion path
- [ ] Persist current node state in SQLite
- [ ] Expose a minimal REST API
- [ ] Ship the first WebUI dashboard
- [ ] Add peer, validator, and alerting workflows
- [ ] Port mature collectors incrementally

## Project principles

- **Greenfield boundaries:** PlatPulse does not inherit the legacy Chaindash architecture.
- **Operational correctness:** freshness, revisions, durable delivery, and alert reliability are first-class concerns.
- **Incremental delivery:** the first milestone is a small end-to-end vertical slice, followed by deeper collectors and richer views.
- **Clear contracts:** shared behavior belongs in explicit, versioned protocol and domain types.

## Contributing

The project is in its foundation phase. Design discussions, issues, and focused pull requests are welcome as the workspace and protocol take shape.

Before publishing integrations or packages under the `PlatPulse` name, perform an independent trademark, domain, GitHub, and package-name availability check.

## License

PlatPulse is licensed under the [MIT License](LICENSE).

Part of the [PlatPulse Server deployment guide](../deployment.md): Build a release bundle; Release-candidate harness.

## Build a release bundle

From the repository root, build a target-aware release set:

```bash
scripts/build-release.sh \
  --target x86_64-unknown-linux-gnu \
  --output target/release-artifacts
```

The set contains separate Agent and Server archives, package-manager artifacts
when the builders are available, checksums, an SPDX inventory, and dependency
audit evidence. The staged Server tree includes the same-origin WebUI and
deployment examples. The staged Agent tree contains the Agent binary, unit, and
configuration reference. Pepper, TLS private keys, Agent credentials,
notification tokens, SQLite state/sidecars, MMDB files, and other secrets are
never included.

`scripts/package-release.sh` remains a compatibility wrapper for the
release-candidate harness and exposes the historical unpacked Server location.

Install one matching architecture using the distribution package manager, or
extract an archive into an empty staging directory before copying its allowlisted
`usr/` and `etc/` trees:

```bash
sudo dpkg -i platpulse-server-<version>-x86_64.deb
sudo dpkg -i platpulse-agent-<version>-x86_64.deb
# or, on RPM-based systems
sudo rpm -Uvh platpulse-server-<version>-1.x86_64.rpm
sudo rpm -Uvh platpulse-agent-<version>-1.x86_64.rpm
```

Native archives do not create service users. Before enabling the included units,
create the documented `platpulse-server`/`platpulse-agent` users, install the
files with root ownership and packaged modes, and create the private state,
secret, and backup directories with the runtime user's ownership.

## Release-candidate harness

Run the packaged artifact through the external CLI, HTTP, and SSE boundaries with:

```bash
scripts/release-candidate-harness.sh
```

The harness creates a unique temporary run directory under
`target/release-candidate-runs/` (override it with
`PLATPULSE_RC_RUNS_ROOT`), builds the release bundle, initializes real temporary
SQLite state, provisions controlled Owner/Viewer/Network fixtures, enrolls an
Agent, submits the canonical two-Node report, and checks the Report Receipt,
Admin projection, WebUI asset, health endpoints, isolated Prometheus metrics,
and authorized Admin SSE.
Normal completion removes the run directory. A failed run removes credentials,
SQLite files, cookies, headers, and response bodies before preserving the
artifact, configuration, logs, request IDs, and sanitized diagnostics. Exit 2
means the environment is unavailable; exit 1 is a harness failure.

For packaged load, fault, and soak evidence, see
[`docs/release-qualification.md`](../release-qualification.md).

To rehearse upgrades and recovery against a packaged Server, run:

```bash
scripts/release-recovery-rehearsal.sh
```

The command creates private schema fixtures under `target/recovery-rehearsal/`,
starts each one so the compiled forward migrations run before serving, and records
sanitized JSON/Markdown evidence. It also verifies the supported backup-artifact
path through the packaged CLI while the Server is stopped; concurrent/live-write
backup is not covered. It then checks checksum and integrity failures,
stopped-Server confirmation, atomic restore and
safety-copy behavior, secret-file preservation, corrupt input refusal, and higher
schema refusal. Use `--self-test` for the fast fixture-generation check.

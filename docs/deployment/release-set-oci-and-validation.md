Part of the [PlatPulse Server deployment guide](../deployment.md): Redeploying a running source checkout; Supported release set; OCI deployment and mount model; Release validation and metadata.

## Redeploying a running source checkout

A Server that runs the binaries directly out of `target/release` reads
`dist/index.html` once at startup and then serves the hashed assets beside it
from disk. Rebuilding the WebUI replaces those hashed files, so a Server left
running keeps handing out an `index.html` that references assets which no
longer exist and the browser renders a blank page. Rebuilding the WebUI without
restarting the Server therefore breaks the WebUI even though every artifact on
disk is correct.

`scripts/deploy-local.sh` rebuilds the Agent/Server binaries and the WebUI, then
restarts the user-level services and proves the live Server resolves every asset
the current build references:

```bash
scripts/deploy-local.sh                 # rebuild, restart if anything changed
scripts/deploy-local.sh --skip-tests    # skip WebUI lint/typecheck/unit tests
scripts/deploy-local.sh --timeout 300   # allow a slower /health/ready budget
scripts/deploy-local.sh --check-only    # report whether the live Server is stale
```

The readiness budget defaults to 180 seconds because a cold start replays the
migrations and opens the projections before the listener binds, which takes over
a minute on a populated database; a tighter budget reports a failed restart while
the services are already healthy. Lower it with `--timeout` only when the state
directory is known to be small.

The restart decision is not limited to "did this run change something": the
script also probes the running Server and restarts it when the live `index.html`
disagrees with `dist/index.html`, or when any asset it references is missing.
That self-heals the stale state above regardless of what rebuilt the WebUI. Run
`--check-only` in monitoring or before a manual rebuild; it exits non-zero when
the live Server is stale. The script never modifies SQLite state, the Owner
account, the enrollment credential, the pepper, or the service configuration.

## Supported release set

The supported release builder is:

```bash
scripts/build-release.sh --target x86_64-unknown-linux-gnu --output target/release-artifacts
```

It produces versioned Server and Agent Linux `x86_64` archives. The release CI runs
that command for both `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`
(the package labels are Debian `amd64`/`arm64` and RPM `x86_64`/`aarch64`). The
artifacts use the Ubuntu 24.04 build baseline and therefore require glibc 2.39 or
newer. They are tested on Ubuntu 24.04 and Fedora 41; Debian 12 and RHEL 9-class
systems are not supported by these GNU-linked packages. The target toolchain and
linker must be installed before requesting an architecture build. `dpkg-deb` and
`rpmbuild` outputs are generated when those builders are available; the build
reports an explicit unavailable status otherwise.

Native archives and packages include the repository `LICENSE` in their package-specific
documentation directories. Each Server archive includes the same-origin WebUI, non-root
systemd units, the Caddy and Compose examples, and the optional MaxMind `geoipupdate` example.
Agent archives include the Agent unit and configuration
reference. Packages install dedicated `platpulse-server` and `platpulse-agent`
system users, create their private state directories plus the Server backup and
`/etc/platpulse/secrets` directories with runtime-user ownership and mode `0700`,
and never enable a service automatically. DEB and RPM scriptlets declare their
`adduser`/`shadow-utils`, `coreutils`, `libc`, and `systemd` runtime dependencies.
Release builds export `SOURCE_DATE_EPOCH` through tar, package, and SBOM generation;
Syft is required for a releasable dependency-aware SPDX SBOM. Fixture and harness
runs may explicitly emit only a non-releasable SBOM-skipped marker.

## OCI deployment and mount model

The OCI build definition is `release/oci/server.Dockerfile`. Its Node, Rust, and
Debian base images are pinned by manifest digest, and it uses no mutable apt
repository resolution. Update those pins only as a reviewed release-input change.
It runs as fixed UID and GID `10001`, declares separate volumes for SQLite state, backup artifacts, secret
files, and optional Geo data. WebUI assets remain in the versioned image layer by
default; operators may explicitly bind-mount a replacement WebUI tree, and the image
does not create an anonymous WebUI volume that could survive an image upgrade. It does
not contain live state or credentials. The
Compose example is `release/compose/server.compose.yml`; copy the accompanying
`release/compose/server.toml` beside it as `server.toml`. It binds inside the
container on `0.0.0.0`, trusts only the pinned Compose subnet for the host HTTPS
reverse proxy, and mounts the Geo database below `/var/lib/platpulse/geo`. Mount
the prepared config and secret directory read-only. Secret files must be regular files owned
by UID `10001` with mode `0600`; bind mounts do not relax the Server's no-symlink
or same-user ownership checks. The image's WebUI is used unless an operator
deliberately mounts a replacement `/usr/share/platpulse/web` tree.

The Geo sidecar example in `release/geo/geoipupdate.compose.yml` uses the official
MaxMind image and operator-provided secrets. PlatPulse does not distribute a
GeoLite database, MaxMind credentials, or a downloader configuration as a
runtime secret. Review MaxMind licensing and provide the resulting MMDB through
the read-only Geo mount.

## Release validation and metadata

`scripts/validate-release.sh` rejects missing executables/WebUI assets, unexpected
or non-regular members, empty directory additions, symlinks, unsafe archive paths,
secret names, SQLite sidecars, MMDB data, Agent state, non-canonical file/directory
modes, and root-running service units. The release
candidate harness runs the unpacked Server archive through the external CLI,
HTTP, WebUI, metrics, AgentReport, and SSE boundaries. Package-manager install
smoke tests run in the release CI's disposable package environments.

Every release set contains `SHA256SUMS`, an SPDX inventory, and recorded Rust/npm
audit evidence. Normal release builds fail when cargo-deny, cargo-audit, or npm audit
is unavailable or exits non-zero; only fixture/harness builds use the explicit audit
skip. The ignored advisories are configured once per tool, in `deny.toml` and
`.cargo/audit.toml`. `RUSTSEC-2023-0071` is ignored because `rsa` is lockfile-only behind disabled
SQLx features (`cargo tree -i rsa` is empty). `RUSTSEC-2024-0436` (`paste`) is
ignored only by cargo-deny because Alloy's proc-macro graph has no maintained
compatible replacement yet. `RUSTSEC-2026-0381` (`webpki-roots` 0.26.11, the
bundled root store Alloy's WebSocket transport pulls through
tokio-tungstenite) is ignored by both gates: the advisory patches only the 1.0
line, every published tungstenite release through 0.30 still requires `^0.26`,
and the workspace's own 1.0 root store is on the patched release.
`RUSTSEC-2026-0387` (`crypto-bigint` 0.5.5, non-constant-time arithmetic) is
ignored by both gates: it arrives through Alloy's k256/elliptic-curve 0.13 stack,
the fix exists only in the incompatible 0.7 line, and PlatPulse never holds a
secret key (the Agent never signs; it only parses public keys and public
signatures). See the residual-risk entries in the security review. The remaining cargo-audit warnings stay visible in
the evidence for release signoff. Checksums and SBOMs are
integrity and inventory metadata only;
they are not artifact signatures. Until an artifact-signing workflow is added,
unsigned artifacts must not be described as a verified supply chain.

The checked-in deployment assets are:

- `release/systemd/` — Server and Agent units;
- `release/examples/Caddyfile` — trusted reverse-proxy example;
- `release/compose/server.compose.yml` and `release/compose/server.toml` — non-root Server Compose example and matching container configuration;
- `release/geo/geoipupdate.compose.yml` — optional Geo sidecar example.

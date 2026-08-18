# PlatPulse Server deployment

A production release contains one runtime process:

```text
/usr/bin/platpulse-server
/usr/share/platpulse/web/
├── index.html
└── assets/
```

The WebUI is not compiled into the Rust binary. It is served same-origin by
`platpulse-server`; no Vite or separate web server is required at runtime.

## Build a release bundle

From the repository root:

```bash
scripts/package-release.sh
```

The script runs the WebUI production build and the Rust release build, then
writes:

```text
target/platpulse-server-<version>.tar.gz
target/release-package/root/usr/bin/platpulse-server
target/release-package/root/usr/share/platpulse/web/
```

An alternate output staging directory may be passed, but it must remain under
`target/`:

```bash
scripts/package-release.sh target/release-package-aarch64
```

The archive contains only the `usr/` installation tree. Pepper, TLS keys,
Agent credentials, notification tokens, and other secrets are not included.

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
Admin projection, WebUI asset, health endpoints, and authorized Admin SSE.
Normal completion removes the run directory. A failed run removes credentials,
SQLite files, cookies, headers, and response bodies before preserving the
artifact, configuration, logs, request IDs, and sanitized diagnostics. Exit 2
means the environment is unavailable; exit 1 is a harness failure.

## Configuration

Copy `crates/platpulse-server/server.example.toml` and set an explicit
`state_dir`, secret paths, and public URL. If `web_root` is omitted, the
Server resolves the installed default `/usr/share/platpulse/web`. A configured
`web_root`, or `serve --web-assets`, takes precedence over that default.

The Server may start when the default or configured WebUI directory is
missing/incomplete, but `/health/ready` reports the `web_assets_missing`
component as not ready. `init` emits a warning instead of creating or
modifying WebUI files.

```bash
platpulse-server init --config /etc/platpulse/server.toml
platpulse-server owner create \
  --config /etc/platpulse/server.toml \
  --username admin
platpulse-server serve --config /etc/platpulse/server.toml
```

`--web-assets /path/to/web` is an explicit `serve` override for tests,
development, and custom installations. It has higher precedence than the
`web_root` value in `server.toml`, which has higher precedence than the
built-in default.

## Transport modes

Production has two explicit transport choices. For direct HTTPS, configure a
certificate chain and private key in `server.toml`:

```toml
listen = "0.0.0.0:8443"
public_base_url = "https://monitor.example.com:8443"
[tls]
cert_chain_file = "/etc/platpulse/tls/fullchain.pem"
private_key_file = "/etc/platpulse/tls/privkey.pem"
```

The private-key path must resolve to a same-user-owned private regular file
with mode `0600`; final symlinks and symlinked ancestors are rejected. The
Server validates and parses both files before binding. A malformed,
unreadable, mismatched, or insecure key fails startup with a redacted
transport diagnostic. PlatPulse does not issue certificates, run ACME, or
reload certificates in-process: replace the files and restart the Server.

Alternatively, terminate HTTPS at a trusted reverse proxy and keep the Server
on a private listener:

```toml
listen = "0.0.0.0:8080"
public_base_url = "https://monitor.example.com"
trusted_proxy_cidrs = ["10.0.0.0/8"]
trusted_proxy_scheme = "https"
```

Forwarded headers are accepted only from a peer in one of the configured CIDRs
and only when the configured scheme is `https`. Conflicting or spoofed
forwarded headers are rejected. A non-loopback production plaintext listener
without either native TLS or this trusted-proxy policy is refused at startup.
Development mode is separate and remains loopback-only HTTP with its
development cookie policy; it cannot be combined with native TLS.

## Internal operational metrics

The optional `[metrics]` section exposes only `GET /metrics` on a dedicated
management listener. It is disabled when the section is absent and defaults to
`127.0.0.1:9090` when enabled without an explicit address:

```toml
[metrics]
enabled = true
listen = "127.0.0.1:9090"
```

The metrics router has no Public, Admin, Agent, health, authentication, or SPA
routes and is not included in OpenAPI or the generated browser client. Its
labels are fixed low-cardinality dimensions; Node IDs, Peer IDs, User IDs,
Agent IDs, IP addresses, report IDs, request parameters, credentials, error
strings, and report bodies are never exposed. Non-loopback metrics binds are
refused unless native Rustls TLS or the explicit trusted HTTPS proxy policy is
configured, using the same pre-bind safety checks as the main listener.

## Same-origin behavior

- `/` and React Router paths such as `/admin` receive `index.html`;
- `/assets/<hashed-file>` receives immutable caching headers;
- `index.html` is served with `Cache-Control: no-cache`;
- unmatched `/api/*` paths remain JSON error responses and never fall through
to the SPA;
- REST, SSE, cookies, and the SPA use the same origin.

## systemd user/service example

Install the binary and WebUI tree using the package manager or release
archive, then create a dedicated `platpulse` service account and protect the
state/config/secret files with that account. A minimal system service is:

```ini
[Unit]
Description=PlatPulse Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=platpulse
Group=platpulse
ExecStart=/usr/bin/platpulse-server serve --config /etc/platpulse/server.toml
Restart=on-failure
RestartSec=5s
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/platpulse

[Install]
WantedBy=multi-user.target
```

For local development, use an explicit `development = true` configuration and
loopback `listen`; do not reuse the development cookie policy in production.
Non-loopback plaintext listeners remain refused until TLS or an explicitly
trusted HTTPS reverse proxy is configured.

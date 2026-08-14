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

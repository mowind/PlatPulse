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

## Where each topic lives

The guide is split by topic into the files under [deployment/](deployment/), so each file stays within
the document budget (`scripts/check-doc-budgets.py`). Every section keeps its heading, so a section cited
by name together with docs/deployment.md (for example "Upgrade and rollback") is the heading listed
here. Release packages install this file and the whole `deployment/` directory side by side under
`/usr/share/doc/platpulse-server/` and `/usr/share/doc/platpulse-agent/`, so the links below also work in
an installed package.

| Sections (› marks a subsection) | File |
|---|---|
| Build a release bundle<br>Release-candidate harness | [release-bundle.md](deployment/release-bundle.md) |
| Configuration<br>› Storage Backend | [configuration-and-storage-backend.md](deployment/configuration-and-storage-backend.md) |
| PostgreSQL Storage Backend<br>› Provisioning the role and the database<br>› TLS, certificate trust, and the password file<br>› One Server per database, and after an ownership loss<br>› Operations that do not apply on PostgreSQL<br>› Storage Snapshots on PostgreSQL are unsanitized host data | [postgresql-storage-backend.md](deployment/postgresql-storage-backend.md) |
| Transport modes<br>Diagnostic logs<br>Internal operational metrics<br>Same-origin behavior<br>Runtime database readiness<br>Durable Spool capacity and the downtime it covers | [transport-logs-metrics-and-spool.md](deployment/transport-logs-metrics-and-spool.md) |
| Agent Store recovery<br>systemd services and offline backups<br>› Node process selectors and supervisor authorization<br>› Offline backups (no packaged automation)<br>› Safe daily backups, verification, and restore rehearsal | [agent-store-systemd-and-backups.md](deployment/agent-store-systemd-and-backups.md) |
| Redeploying a running source checkout<br>Supported release set<br>OCI deployment and mount model<br>Release validation and metadata | [release-set-oci-and-validation.md](deployment/release-set-oci-and-validation.md) |
| Upgrade and rollback<br>› Coordinated Inventory v2 cutover (issue #192)<br>› Storage Backend Migration (issue #257) | [upgrade-and-rollback.md](deployment/upgrade-and-rollback.md) |
| Node Network Identity at onboarding and after a Node build upgrade<br>› When the observation matches but the Block history is still rejected<br>› When a Node answers millisecond timestamps | [node-network-identity.md](deployment/node-network-identity.md) |

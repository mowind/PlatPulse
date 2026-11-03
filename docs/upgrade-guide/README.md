# Upgrade guides

This directory holds one operator-facing upgrade guide per release, in a subdirectory named `v<version>/` (for example [`v0.3.0/`](v0.3.0/README.md)). Each guide lists what an operator must know and do when moving from the previous release: backups and upgrade order, breaking changes, schema migrations, configuration, CLI and API changes, and rollback.

Rule: every externally visible breaking change — to configuration, the CLI, an on-disk data format or schema, Agent–Server version compatibility, or the HTTP API — is written into the next release's guide in the same pull request that makes the change.

The guide for an unreleased version is a draft until its release tag is created.

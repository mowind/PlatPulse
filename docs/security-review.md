# Phase 5 security review

Issue #73 is qualified through the packaged release boundary. The release-candidate harness builds and validates the Server, Agent, and WebUI artifacts, starts the packaged Server with native TLS, and exercises the HTTP/SSE/metrics interfaces. It records only sanitized evidence and fails on an unresolved release-blocking finding.

Run the external review with:

```bash
scripts/release-candidate-harness.sh
```

The harness is the executable evidence for the matrix below; the checked-in dispositions and owners live in `release/qualification/security.toml`. Rust/WebUI tests provide focused coverage where a safe black-box mutation would be destructive or environment-dependent.

## Security matrix

| Area | Executable evidence | Disposition |
| --- | --- | --- |
| Public/Admin/Agent route authorization | Packaged HTTP requests with Guest, Viewer, Owner, and Agent credentials | PASS: route groups reject the wrong principal and do not cross credential boundaries. |
| Guest access and private projections | Anonymous requests with Guest access disabled plus authenticated Public projection assertions | PASS for the packaged default-private boundary; Guest-enabled transition remains a residual fixture gap. |
| Session fixation and revocation | Packaged login with an existing cookie and logout; focused role/disable tests | PASS for rotation and logout. Role/disable transitions remain an explicit nonblocking residual risk. |
| CSRF, exact Origin, cookie policy | Missing/wrong CSRF, wrong Origin, Secure/HttpOnly/SameSite cookie and TLS header checks | PASS: mutations and login require the configured security signals. |
| Trusted proxy and native TLS | Forwarded-header spoofing, conflicting schemes, non-loopback plaintext, invalid/insecure/symlinked TLS key checks | PASS: untrusted or insecure transport is rejected. |
| Enrollment and Recovery | Packaged Enrollment exchange and reuse rejection | PASS for Enrollment; Recovery exchange/reuse and old-credential revocation remain NOT_RUN. |
| Limits and parser failures | Malformed JSON, bounded error envelopes, encoded traversal, and API fallback | PASS for exercised probes; unknown enum, oversized login, and complete list-limit checks remain NOT_RUN. |
| Injection and routing | SQL-shaped identifiers, encoded traversal, API-not-found versus SPA fallback, and external redirect probes | PASS: identifiers remain data; no traversal or redirect was observed. |
| Redaction and operations | Metrics, backup mode, recovery rehearsal, and sanitized error probes | PASS for exercised outputs; recursive logs/Doctor/package/restore scan remains NOT_RUN. |
| Dependencies and package contents | Package validation and configured audit commands | Per-finding dependency disposition is a release-environment residual risk and is not claimed as run here. |
| Optional integrations | Geo, Validator Provider, notifications, and individual Agent failure checks | PASS: failures remain degraded and do not cross trust/readiness boundaries. |

## Residual risks and owners

- **Release Engineering:** capacity and resource observations remain specific to the recorded artifact, host, profile, and duration.
- **Server maintainers:** real PlatON RPC behavior and third-party notification providers are not represented by loopback fixtures.
- **Security owner:** dependency advisories that are not exploitable in the shipped feature set remain tracked by the release dependency gate; they must not be silently presented as clean.
- **Operations owner:** production proxy topology, filesystem ownership, and native package installation must be rechecked for each deployment environment.

A missing environment-dependent check is reported as `NOT_RUN`, never as `PASS`. A release-blocking failure prevents qualification and issue closure.

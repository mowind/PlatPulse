#!/usr/bin/env bash
# Prove sensitive files are excluded from the Docker build context.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SEAM="$ROOT/.docker-context-seam"
IMAGE="platpulse-docker-context-test:local"
cleanup() {
  rm -rf "$SEAM"
  docker image rm -f "$IMAGE" >/dev/null 2>&1 || true
}
trap cleanup EXIT
rm -rf "$SEAM"
mkdir -p "$SEAM/secrets"
printf 'safe context sentinel\n' > "$SEAM/safe.txt"
for secret in private.key tls.pem identity.p12 signing.pfx keystore.jks .env.production id_rsa id_ed25519 owner-token agent-credential server-pepper credentials.json token.json live.sqlite live.sqlite3 live.wal live.shm; do
  printf 'must-not-enter-context\n' > "$SEAM/$secret"
done
printf 'must-not-enter-context\n' > "$SEAM/secrets/arbitrary-name"
cat > "$SEAM/Dockerfile" <<'EOF'
FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241
COPY . /context
RUN test -f /context/.docker-context-seam/safe.txt \
 && test -z "$(find /context/.docker-context-seam -type f ! -name safe.txt ! -name Dockerfile -print -quit)"
EOF

docker build --no-cache -f "$SEAM/Dockerfile" -t "$IMAGE" "$ROOT" >/dev/null
printf 'Docker build-context exclusion tests: PASS\n'

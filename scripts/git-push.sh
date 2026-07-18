#!/usr/bin/env bash
# git-push.sh — push to origin without storing the token in .git/config.
#
# Usage:
#   scripts/git-push.sh [refspec...]   # default: origin main
#
# Reads GITHUB_TOKEN from /opt/data/.env and injects it as
# https://x-access-token:<TOKEN>@github.com/... for THIS push only via
# `git push -c remote.origin.url=...`. The .git/config remote.origin.url
# stays clean (no embedded credentials).
#
# Requires HTTPS_PROXY (set below) for GFW-fltered outbound.
set -euo pipefail

REPO=/opt/data/home/ccm
ENV_FILE=/opt/data/.env
PROXY=socks5://192.168.8.3:7890

# Load GITHUB_TOKEN from .env (only this one var).
if [[ ! -f "$ENV_FILE" ]]; then
  echo "ERROR: $ENV_FILE not found" >&2; exit 1
fi
TOKEN=$(sed -nE 's/^GITHUB_TOKEN=["'"'"']?([^"'"'"' ]+)["'"'"']?$/\1/p' "$ENV_FILE" | head -1)
if [[ -z "$TOKEN" ]]; then
  echo "ERROR: GITHUB_TOKEN not found in $ENV_FILE" >&2; exit 1
fi

cd "$REPO"
export HTTPS_PROXY="$PROXY" HTTP_PROXY="$PROXY"

REFSPEC=("$@")
[[ ${#REFSPEC[@]} -eq 0 ]] && REFSPEC=("main")

INJECT_URL="https://x-access-token:${TOKEN}@github.com/winffychu/claude-code-mux.git"

echo "Pushing ${REFSPEC[*]} to origin (token injected, .git/config stays clean)..."
exec git push -c "remote.origin.url=${INJECT_URL}" origin "${REFSPEC[@]}"

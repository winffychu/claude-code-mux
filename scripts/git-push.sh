#!/usr/bin/env bash
# git-push.sh — push to origin without storing the token in .git/config.
#
# Usage:
#   scripts/git-push.sh [refspec...]   # default: main
#
# Reads GITHUB_TOKEN from /opt/data/.env and the clean remote.origin.url from
# .git/config (which must NOT contain embedded credentials), then injects the
# token as https://x-access-token:<TOKEN>@<host>/<path> for THIS push only by
# passing the URL as the repository argument. The .git/config remote.origin.url
# stays clean (no embedded credentials).
#
# Requires HTTPS_PROXY (set below) for GFW-filtered outbound.
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

# Read the clean origin URL (must NOT already contain credentials).
CLEAN_URL=$(git config --get remote.origin.url)
if [[ "$CLEAN_URL" =~ [a-zA-Z0-9_]://[^/]*@ ]]; then
  echo "ERROR: remote.origin.url already contains embedded credentials. Clean it first:" >&2
  echo "  git remote set-url origin \$(echo \"\$CLEAN_URL\" | sed -E 's#://[^@]*@#://#')" >&2
  exit 1
fi
if [[ -z "$CLEAN_URL" ]]; then
  echo "ERROR: remote.origin.url not set in .git/config" >&2; exit 1
fi

# Inject the token into the URL: scheme://host/path → scheme://x-access-token:TOKEN@host/path
INJECT_URL=$(echo "$CLEAN_URL" | sed -E "s#^(https?://)#\1x-access-token:${TOKEN}@#")

REFSPEC=("$@")
[[ ${#REFSPEC[@]} -eq 0 ]] && REFSPEC=("main")

echo "Pushing ${REFSPEC[*]} (token injected, .git/config stays clean)..."
exec git push "$INJECT_URL" "${REFSPEC[@]}"

#!/usr/bin/env bash
# Submits a .zip, .dmg or .pkg to Apple's notary service and waits for the verdict.
#   scripts/notarize.sh dist/Poo.app.zip
# Credentials, first match wins:
#   APPLE_API_KEY_P8 (base64 .p8) + APPLE_API_KEY_ID + APPLE_API_ISSUER   App Store Connect API key
#   APPLE_ID + APPLE_TEAM_ID + APPLE_APP_PASSWORD                         app-specific password
#   NOTARY_PROFILE                                    `xcrun notarytool store-credentials` profile
# Exits 3 when no credentials are set, so callers can skip notarization cleanly.
set -euo pipefail
file="${1:?usage: notarize.sh FILE}"

args=()
if [ -n "${APPLE_API_KEY_P8:-}" ]; then
  key="$(mktemp -d)/AuthKey.p8"
  printf '%s' "$APPLE_API_KEY_P8" | base64 --decode > "$key"
  trap 'rm -f "$key"' EXIT
  args=(--key "$key" --key-id "${APPLE_API_KEY_ID:?}" --issuer "${APPLE_API_ISSUER:?}")
elif [ -n "${APPLE_ID:-}" ]; then
  args=(--apple-id "$APPLE_ID" --team-id "${APPLE_TEAM_ID:?}" --password "${APPLE_APP_PASSWORD:?}")
elif [ -n "${NOTARY_PROFILE:-}" ]; then
  args=(--keychain-profile "$NOTARY_PROFILE")
else
  echo "notarize: no credentials" >&2
  exit 3
fi

out="$(xcrun notarytool submit "$file" "${args[@]}" --wait --output-format json)"
echo "$out"
id="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("id",""))' "$out")"
status="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1]).get("status",""))' "$out")"
if [ "$status" != "Accepted" ]; then
  echo "notarization of $file: $status" >&2
  [ -n "$id" ] && xcrun notarytool log "$id" "${args[@]}" >&2 || true
  exit 1
fi
echo "notarized $file ($id)"

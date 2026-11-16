# Releasing

For maintainers. Users only need the [install steps in the README](../README.md#install).

## How releases happen

Pushing to `main` runs CI only. A release ships when a version tag is pushed:

```sh
scripts/release.sh            # tags origin/main with the next patch and pushes the tag
scripts/release.sh 0.2.0      # or an explicit version
scripts/release.sh --dry-run  # show the version and the changes since the last tag
```

[`.github/workflows/release.yml`](../.github/workflows/release.yml) then waits for the
`ci` run on that commit to pass (Rust tests on macOS and Linux, `mods validate`, the app
build and tests), builds, signs, notarizes and publishes. Running the workflow by hand
releases `main`'s head with the next patch. Batch changes into one release rather than
tagging every push; installed apps and CLIs pick it up silently.

**Versioning.** `major.minor` come from `Cargo.toml`; the patch counts up from the
highest `v<major>.<minor>.*` tag (the first release uses `Cargo.toml`'s patch), unless you
pass a version. The
version is stamped into the build (`scripts/stamp-version.sh`), never committed; the
release tag `vX.Y.Z` on the commit records it. Bump `major.minor` in `Cargo.toml` to
start a new series.

**Assets.** `poo-<target>.tar.gz` for macOS and Linux (arm64, x86_64), a universal
`Poo.app.zip`, `latest.json` (read by `install.sh` and `poo update`), `appcast.xml`
(read by Sparkle) and `SHA256SUMS`.

## Secrets

Set these in GitHub under Settings > Secrets and variables > Actions. Without them a
release still publishes, with less in it.

| Secret | What | Without it |
|---|---|---|
| `SPARKLE_PRIVATE_KEY` | Sparkle EdDSA private key (base64 seed, below) | No `appcast.xml`; the app in that release has no `SUPublicEDKey` and never updates itself |
| `MACOS_CERT_P12` | Developer ID Application certificate and key, `.p12`, base64 | App is ad-hoc signed |
| `MACOS_CERT_PASSWORD` | Password of that `.p12` | |
| `MACOS_SIGN_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` | |
| `APPLE_ID` | Apple ID email used for notarization | Not notarized |
| `APPLE_TEAM_ID` | 10-character team id | |
| `APPLE_APP_PASSWORD` | App-specific password from appleid.apple.com | |
| `APPLE_API_KEY_P8` / `APPLE_API_KEY_ID` / `APPLE_API_ISSUER` | Instead of the three above: an App Store Connect API key (`.p8`, base64), its key id and issuer id | |

## Sparkle key

Once. `generate_keys` comes with Sparkle:

```sh
swift package --package-path app resolve
app/.build/artifacts/sparkle/Sparkle/bin/generate_keys            # creates the key in your login keychain, prints the public key
app/.build/artifacts/sparkle/Sparkle/bin/generate_keys -x sparkle-private-key.txt
gh secret set SPARKLE_PRIVATE_KEY < sparkle-private-key.txt
rm sparkle-private-key.txt
```

The private key lives in your login keychain. Back it up: losing it means shipped apps
can only update again through a Developer ID signed build. The public key is not stored
anywhere; the release derives it from the secret (`scripts/sparkle.sh pubkey`).

## Developer ID signing and notarization

Optional. Signs Poo.app, its bundled CLI and the macOS CLI binaries, notarizes both and
staples the app.

1. In Keychain Access > login > My Certificates, right-click "Developer ID Application:
   Your Name (TEAMID)" and export it as `cert.p12` with a password. (No certificate yet:
   Xcode > Settings > Accounts > Manage Certificates > + > Developer ID Application.)
2. Notarization credentials, either:
   - an app-specific password: appleid.apple.com > Sign-In and Security > App-Specific
     Passwords, plus your Apple ID email and team id; or
   - an App Store Connect API key: App Store Connect > Users and Access > Integrations >
     Team Keys, role Developer; download `AuthKey_<id>.p8` and note the key id and issuer id.
3. Store them:

```sh
base64 -i cert.p12 | gh secret set MACOS_CERT_P12
gh secret set MACOS_CERT_PASSWORD             # the export password
gh secret set MACOS_SIGN_IDENTITY --body "Developer ID Application: Your Name (TEAMID)"
# app-specific password route
gh secret set APPLE_ID --body "you@example.com"
gh secret set APPLE_TEAM_ID --body "TEAMID"
gh secret set APPLE_APP_PASSWORD             # xxxx-xxxx-xxxx-xxxx
# or the API key route
base64 -i AuthKey_ABC123.p8 | gh secret set APPLE_API_KEY_P8
gh secret set APPLE_API_KEY_ID --body "ABC123"
gh secret set APPLE_API_ISSUER --body "<issuer uuid>"
rm cert.p12
```

Locally, `SIGN_IDENTITY="Developer ID Application: ..." ./scripts/build-app.sh` signs,
and `NOTARY_PROFILE=<profile> scripts/notarize.sh dist/Poo.app.zip` notarizes with a
profile saved by `xcrun notarytool store-credentials`.

## Ad-hoc signed builds and Sparkle

Sparkle 2 accepts an update when its EdDSA signature verifies against the running app's
key and the new app's code signature is valid; it does not require the old and new
designated requirements to match when EdDSA passes. So ad-hoc builds update each other,
and moving to Developer ID later is a normal update.

What ad-hoc costs: Gatekeeper blocks the first launch of a download (right-click > Open,
or `xattr -dr com.apple.quarantine /Applications/Poo.app`), and macOS ties privacy grants
such as Full Disk Access to the exact build, so each ad-hoc update asks for them again.
Ad-hoc builds are signed without the hardened runtime, which would refuse to load the
ad-hoc signed Sparkle framework.

#!/bin/bash
# Builds, notarizes and packages a release into dist/:
#   Meridian-<version>.dmg   signed, notarized and stapled disk image
#   Meridian-<version>.zip   the stapled app, zipped
#   SHA256SUMS.txt
#
#   scripts/release.sh             # build, notarize, package
#   scripts/release.sh --publish   # also create the GitHub release v<version>
#                                  # (notes from docs/release-notes/<version>.md)
#
# Notarization uses a notarytool keychain profile, created once with:
#   xcrun notarytool store-credentials meridian-notary \
#     --apple-id "<your Apple ID>" --team-id H7T2D2GL7U
# (it prompts for an app-specific password and stores it in the Keychain).
# Override the profile with NOTARY_PROFILE and the identity with SIGN_IDENTITY.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${NOTARY_PROFILE:-meridian-notary}"
IDENTITY="${SIGN_IDENTITY:-Developer ID Application}"
PUBLISH=0
for a in "$@"; do
  case "$a" in
    --publish) PUBLISH=1 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done

step() { printf '\n== %s\n' "$*"; }
fail() { echo "release: $*" >&2; exit 1; }

step "Checking notary credentials ($PROFILE)"
xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1 \
  || fail "no notarytool profile '$PROFILE'; create it with: xcrun notarytool store-credentials $PROFILE --apple-id \"<your Apple ID>\" --team-id H7T2D2GL7U"

step "Building the release app"
"$ROOT/scripts/build-app.sh" release
APP="$ROOT/app/build/DerivedData/Build/Products/Release/Meridian.app"
VERSION=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$APP/Contents/Info.plist")
DIST="$ROOT/dist"
rm -rf "$DIST"
mkdir -p "$DIST"

step "Checking the signature"
codesign --verify --deep --strict "$APP"
codesign -dvv "$APP" 2>&1 | grep -q "Authority=Developer ID Application" || fail "not signed with a Developer ID Application certificate"
codesign -dvv "$APP" 2>&1 | grep -q "^Timestamp=" || fail "signature has no secure timestamp"
codesign -dvv "$APP" 2>&1 | grep -q "(runtime)" || fail "hardened runtime is off"
if codesign -d --entitlements - --xml "$APP" 2>/dev/null | grep -q "get-task-allow"; then
  fail "the app has the get-task-allow entitlement (a debug build?)"
fi

# Submits a file and waits; on rejection prints Apple's log and stops.
notarize() {
  local file="$1" out id status
  out=$(xcrun notarytool submit "$file" --keychain-profile "$PROFILE" --wait --output-format json)
  id=$(printf '%s' "$out" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')
  status=$(printf '%s' "$out" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')
  echo "notarization $id: $status"
  if [ "$status" != "Accepted" ]; then
    [ -n "$id" ] && xcrun notarytool log "$id" --keychain-profile "$PROFILE" || true
    fail "notarization of $(basename "$file") was not accepted"
  fi
}

step "Notarizing the app"
ditto -c -k --keepParent "$APP" "$DIST/notarize.zip"
notarize "$DIST/notarize.zip"
rm "$DIST/notarize.zip"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl -a -t exec -vv "$APP" 2>&1 | tee /dev/stderr | grep -q "source=Notarized Developer ID" || fail "Gatekeeper does not accept the app"

step "Packaging"
ditto -c -k --keepParent "$APP" "$DIST/Meridian-$VERSION.zip"
# The disk image's window: background drawn in the app's palette, icons
# placed by dmgbuild (writes the window layout without scripting Finder).
VENV="$ROOT/app/build/.venv-dmg"
if [ ! -x "$VENV/bin/dmgbuild" ]; then
  python3 -m venv "$VENV"
  "$VENV/bin/pip" install -q "dmgbuild==1.6.7" "ds_store==1.3.3" "mac_alias==2.2.3"
fi
BG="$(mktemp -d)"
swift "$ROOT/scripts/make-dmg-background.swift" "$VERSION" "$BG" >/dev/null
"$VENV/bin/dmgbuild" -s "$ROOT/scripts/dmg/settings.py" -D app="$APP" -D background="$BG/background.png" \
  "Meridian $VERSION" "$DIST/Meridian-$VERSION.dmg" 2>&1 | grep -v "is deprecated" || true
rm -rf "$BG"
[ -f "$DIST/Meridian-$VERSION.dmg" ] || fail "dmgbuild did not produce the disk image"
codesign --sign "$IDENTITY" --timestamp "$DIST/Meridian-$VERSION.dmg"

step "Notarizing the disk image"
notarize "$DIST/Meridian-$VERSION.dmg"
xcrun stapler staple "$DIST/Meridian-$VERSION.dmg"
xcrun stapler validate "$DIST/Meridian-$VERSION.dmg"
spctl -a -t open --context context:primary-signature -vv "$DIST/Meridian-$VERSION.dmg" 2>&1 | tee /dev/stderr \
  | grep -q "source=Notarized Developer ID" || fail "Gatekeeper does not accept the disk image"

(cd "$DIST" && shasum -a 256 "Meridian-$VERSION.dmg" "Meridian-$VERSION.zip" > SHA256SUMS.txt)
step "Done: $DIST"
ls -l "$DIST"
cat "$DIST/SHA256SUMS.txt"

if [ "$PUBLISH" = 1 ]; then
  NOTES="$ROOT/docs/release-notes/$VERSION.md"
  [ -f "$NOTES" ] || fail "missing $NOTES"
  step "Publishing v$VERSION"
  gh release create "v$VERSION" "$DIST/Meridian-$VERSION.dmg" "$DIST/Meridian-$VERSION.zip" "$DIST/SHA256SUMS.txt" \
    --target main --title "Meridian $VERSION" --notes-file "$NOTES" --latest
fi

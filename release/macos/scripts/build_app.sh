#!/usr/bin/env bash
# 构建官网下载版 DiffCode.app：Developer ID 签名（由内向外，不用 --deep）、Hardened Runtime、公证并装订票据，
# 然后打成 Sparkle 用的 .app.zip。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

APP_NAME="${APP_NAME:-DiffCode}"
DIST_DIR="${DIST_DIR:-$PROJECT_ROOT_DIR/dist/macos}"
TEAM_ID="${TEAM_ID:-}"
DEVELOPER_ID_APP="${DEVELOPER_ID_APP:-}"
APP_ENTITLEMENTS="${APP_ENTITLEMENTS:-$RELEASE_MACOS_DIR/DiffCode.entitlements}"
NOTARIZE_APP="${NOTARIZE_APP:-1}"
CHECK_GATEKEEPER="${CHECK_GATEKEEPER:-1}"

if [[ "$DIST_DIR" != /* ]]; then
  DIST_DIR="$PROJECT_ROOT_DIR/$DIST_DIR"
fi
if [[ "$APP_ENTITLEMENTS" != /* ]]; then
  APP_ENTITLEMENTS="$PROJECT_ROOT_DIR/$APP_ENTITLEMENTS"
fi

require_command swift
require_command ditto
require_command shasum
require_command codesign
require_command lipo

if [[ -n "$TEAM_ID" && -z "$DEVELOPER_ID_APP" ]]; then
  DEVELOPER_ID_APP="$(resolve_identity "Developer ID Application" "$TEAM_ID")"
fi
[[ -n "$DEVELOPER_ID_APP" ]] || fail "cannot find a Developer ID Application certificate for team ${TEAM_ID:-<unset>}"
[[ -f "$APP_ENTITLEMENTS" ]] || fail "release entitlements not found: $APP_ENTITLEMENTS"
print_step "Using app identity: $DEVELOPER_ID_APP"

app_dist_path="$DIST_DIR/$APP_NAME.app"
mkdir -p "$DIST_DIR"

print_step "Building and signing $APP_NAME.app"
ARCH="${ARCH:-arm64}" \
EDITION=direct \
OUTPUT_DIR="$DIST_DIR" \
SIGN_IDENTITY="$DEVELOPER_ID_APP" \
HARDENED_RUNTIME=1 \
ENTITLEMENTS="$APP_ENTITLEMENTS" \
"$PROJECT_ROOT_DIR/Scripts/build-app.sh"

# 默认是 Apple Silicon 版；Intel 版（ARCH=x86_64）单独构建、单独下载，好让前者保持小巧。
expected_archs="${ARCH:-arm64}"
[[ "${UNIVERSAL:-0}" == "1" ]] && expected_archs="arm64 x86_64"
sorted_words() { tr ' ' '\n' | sed '/^$/d' | sort | xargs; }
actual_archs="$(lipo -archs "$app_dist_path/Contents/MacOS/$APP_NAME" | sorted_words)"
[[ "$actual_archs" == "$(sorted_words <<<"$expected_archs")" ]] \
  || fail "the app is built for $actual_archs, not $expected_archs"
print_step "Architecture: $actual_archs"

info_plist="$app_dist_path/Contents/Info.plist"
bundle_id="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$info_plist")"
feed_url="$(/usr/bin/plutil -extract SUFeedURL raw -o - "$info_plist" 2>/dev/null || true)"
public_key="$(/usr/bin/plutil -extract SUPublicEDKey raw -o - "$info_plist" 2>/dev/null || true)"
# 发出去的版本如果不带更新地址，用户就永远停在这一版了。
[[ -n "$feed_url" ]] || fail "SUFeedURL is missing: fill in release/macos/AppInfo.local.env (see AppInfo.local.env.example)"
[[ -n "$public_key" && "$public_key" != YOUR_* ]] || fail "SUPublicEDKey is missing: fill in release/macos/AppInfo.local.env"
print_step "Sparkle feed: $feed_url"

# 应用必须是自包含的：不能依赖 Homebrew 的动态库。
while IFS= read -r -d '' candidate; do
  linked="$(otool -L "$candidate" 2>/dev/null | awk 'NR > 1 { print $1 }' || true)"
  if grep -qE '^(/opt/homebrew|/usr/local)/' <<<"$linked"; then
    fail "$candidate links a library outside the app bundle (Homebrew)"
  fi
done < <(find "$app_dist_path/Contents" -type f -print0)

print_step "Verifying signatures and entitlements"
codesign --verify --deep --strict --verbose=2 "$app_dist_path"
signed_entitlements="$(mktemp -t dif-signed-entitlements)"
codesign -d --entitlements :- "$app_dist_path" > "$signed_entitlements" 2>/dev/null || true
get_task_allow="$(/usr/libexec/PlistBuddy -c "Print :com.apple.security.get-task-allow" "$signed_entitlements" 2>/dev/null || true)"
sandbox_enabled="$(/usr/libexec/PlistBuddy -c "Print :com.apple.security.app-sandbox" "$signed_entitlements" 2>/dev/null || true)"
rm -f "$signed_entitlements"
[[ "$get_task_allow" != "true" ]] || fail "signed release app must not include com.apple.security.get-task-allow=true"
# 官网下载版不进沙盒；如果哪天进了，Sparkle 需要 Installer XPC 和对应的 mach-lookup 权限。
if [[ "$sandbox_enabled" == "true" ]]; then
  launcher="$(/usr/bin/plutil -extract SUEnableInstallerLauncherService raw -o - "$info_plist" 2>/dev/null || true)"
  [[ "$launcher" == "true" ]] || fail "sandboxed app is missing SUEnableInstallerLauncherService=true"
  host_entitlements="$(codesign -d --entitlements :- "$app_dist_path" 2>/dev/null || true)"
  grep -Fq "$bundle_id-spks" <<<"$host_entitlements" || fail "sandboxed app is missing the $bundle_id-spks mach-lookup entitlement"
  grep -Fq "$bundle_id-spki" <<<"$host_entitlements" || fail "sandboxed app is missing the $bundle_id-spki mach-lookup entitlement"
fi
installer_xpc="$app_dist_path/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices/Installer.xpc"
installer_entitlements="$(codesign -d --entitlements :- "$installer_xpc" 2>/dev/null || true)"
if grep -q "com.apple.security.app-sandbox" <<<"$installer_entitlements"; then
  fail "Sparkle Installer.xpc must not be sandboxed"
fi
for nested in "$installer_xpc" "$app_dist_path/Contents/Frameworks/Sparkle.framework/Versions/B/Autoupdate" \
              "$app_dist_path/Contents/Frameworks/Sparkle.framework/Versions/B/Updater.app" "$app_dist_path/Contents/Frameworks/Sparkle.framework"; do
  signature_details="$(codesign -dvv "$nested" 2>&1 || true)"
  grep -q "^TeamIdentifier=$TEAM_ID\$" <<<"$signature_details" || fail "not signed by team $TEAM_ID: $nested"
done

app_zip_path="$DIST_DIR/$APP_NAME.app.zip"
if [[ "$NOTARIZE_APP" == "1" ]]; then
  has_notary_auth || fail "notarization credentials are missing (NOTARY_PROFILE or NOTARY_KEY_*); set NOTARIZE_APP=0 to skip"
  print_step "Submitting the app for notarization"
  rm -f "$app_zip_path"
  ditto -c -k --sequesterRsrc --keepParent "$app_dist_path" "$app_zip_path"
  notarize_and_wait "$app_zip_path"
  print_step "Stapling the notarization ticket to the app"
  xcrun stapler staple -v "$app_dist_path"
  xcrun stapler validate "$app_dist_path"
fi

if [[ "$CHECK_GATEKEEPER" == "1" && "$NOTARIZE_APP" == "1" ]]; then
  print_step "Checking Gatekeeper"
  spctl --assess --type execute --verbose=4 "$app_dist_path"
fi

print_step "Creating the update archive"
rm -f "$app_zip_path"
ditto -c -k --sequesterRsrc --keepParent "$app_dist_path" "$app_zip_path"
(
  cd "$DIST_DIR"
  shasum -a 256 "$(basename "$app_zip_path")" > SHA256SUMS.txt
)

print_step "Release app artifact is ready"
echo "APP: $app_dist_path"
echo "ZIP: $app_zip_path"

#!/usr/bin/env bash
# Mac App Store 版：构建沙盒化的通用二进制 DiffCode.app（Apple Distribution 签名，嵌入描述文件），
# 打成 Mac Installer Distribution 签名的 .pkg，再让 App Store Connect 预检（VALIDATE=0 跳过）。不上传。
#
# 先运行一次 release/appstore/scripts/setup_signing.sh（注册 bundle ID、证书、描述文件）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"
load_appstore_env

APP_NAME="${APP_NAME:-DiffCode}"
VALIDATE="${VALIDATE:-1}"
UNIVERSAL="${UNIVERSAL:-1}"
app_path="$DIST_DIR/$APP_NAME.app"

# Signing identities: the valid certificate of each kind (the keychain also holds a revoked Apple Distribution one,
# so the app is signed by SHA-1, never by the ambiguous name).
dist_line="${APP_SIGN_IDENTITY:-$(valid_identity "Apple Distribution" "$TEAM_ID")}"
app_identity="$(cut -f1 <<<"$dist_line")"
[[ -n "$app_identity" ]] || fail "no valid Apple Distribution certificate for team $TEAM_ID in the keychain"
print_step "App identity: $(cut -f2 <<<"$dist_line") (${app_identity:0:8}…)"

[[ -f "$PROVISIONING_PROFILE" ]] || fail "provisioning profile not found: $PROVISIONING_PROFILE (run release/appstore/scripts/setup_signing.sh)"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/diffcode-appstore.XXXXXX")"
trap 'rm -rf "$work_dir"' EXIT
profile_plist="$work_dir/profile.plist"
entitlements="$work_dir/entitlements.plist"
security cms -D -i "$PROVISIONING_PROFILE" > "$profile_plist" 2>/dev/null || fail "cannot decode $PROVISIONING_PROFILE"

# The profile must be a Mac App Store one, for this app, still valid, and carry the signing certificate.
profile_value() { /usr/libexec/PlistBuddy -c "Print :$1" "$profile_plist" 2>/dev/null || true; }
expected_app_id="$TEAM_ID.$BUNDLE_ID"
[[ "$(profile_value "Entitlements:com.apple.application-identifier")" == "$expected_app_id" ]] \
  || fail "the profile is for $(profile_value "Entitlements:com.apple.application-identifier"), not $expected_app_id"
[[ -z "$(profile_value ProvisionedDevices)" && "$(profile_value ProvisionsAllDevices)" != "true" ]] \
  || fail "the profile is a development or Developer ID one; App Store needs a MAC_APP_STORE profile"
expiration="$(profile_value ExpirationDate)"
[[ -n "$expiration" ]] && (( $(date -j -f "%a %b %d %T %Z %Y" "$expiration" +%s 2>/dev/null || echo 0) > $(date +%s) )) \
  || fail "the profile has expired ($expiration); run setup_signing.sh again"
profile_has_certificate "$profile_plist" "$app_identity" || fail "the profile does not include the Apple Distribution certificate ${app_identity:0:8}…; run setup_signing.sh again"
print_step "Profile: $(profile_value Name), valid until $expiration"

# Entitlements: the sandbox ones, plus the identity the profile grants.
cp "$APPSTORE_DIR/DiffCode.entitlements" "$entitlements"
/usr/libexec/PlistBuddy -c "Add :com.apple.application-identifier string $expected_app_id" "$entitlements"
/usr/libexec/PlistBuddy -c "Add :com.apple.developer.team-identifier string $TEAM_ID" "$entitlements"

print_step "Building and signing $APP_NAME.app (universal: $UNIVERSAL)"
EDITION=appstore \
UNIVERSAL="$UNIVERSAL" \
OUTPUT_DIR="$DIST_DIR" \
SIGN_IDENTITY="$app_identity" \
ENTITLEMENTS="$entitlements" \
PROVISIONING_PROFILE="$PROVISIONING_PROFILE" \
"$PROJECT_ROOT_DIR/Scripts/build-app.sh"

print_step "Checking the signed app"
codesign --verify --deep --strict --verbose=2 "$app_path"
signed="$(codesign -d --entitlements :- "$app_path" 2>/dev/null || true)"
signed_value() { /usr/libexec/PlistBuddy -c "Print :$1" /dev/stdin <<<"$signed" 2>/dev/null || true; }
[[ "$(signed_value com.apple.security.app-sandbox)" == "true" ]] || fail "the app is not sandboxed"
[[ "$(signed_value com.apple.application-identifier)" == "$expected_app_id" ]] || fail "wrong application identifier in the signature"
[[ "$(signed_value com.apple.security.get-task-allow)" != "true" ]] || fail "get-task-allow must not ship"
[[ ! -e "$app_path/Contents/Frameworks/Sparkle.framework" ]] || fail "Sparkle must not be in the App Store edition"
[[ -f "$app_path/Contents/embedded.provisionprofile" ]] || fail "the provisioning profile is not embedded"
lipo -archs "$app_path/Contents/MacOS/$APP_NAME"

installer_line="${INSTALLER_SIGN_IDENTITY:-$(valid_identity "3rd Party Mac Developer Installer" "$TEAM_ID")}"
installer_name="$(cut -f2 <<<"$installer_line")"
[[ -n "$installer_name" ]] || fail "no Mac Installer Distribution certificate in the keychain (run release/appstore/scripts/setup_signing.sh)"

# The App Store installs the app itself, so this is the plain component package Xcode makes too; the root-based
# payload the direct download's .pkg needs (against Installer relocating the app) does not apply here.
version="$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$app_path/Contents/Info.plist")"
build="$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$app_path/Contents/Info.plist")"
pkg_path="$DIST_DIR/$APP_NAME-$version-$build-AppStore.pkg"
print_step "Packaging $(basename "$pkg_path") with $installer_name"
rm -f "$pkg_path"
productbuild --component "$app_path" /Applications --sign "$installer_name" "$pkg_path"
pkgutil --check-signature "$pkg_path" | head -n 3

if [[ "$VALIDATE" == "1" ]]; then
  require_asc_key
  echo "PKG: $pkg_path"
  # Validation checks the package against the app's record, so it needs one.
  app_id="$(app_record_id)"
  [[ -n "$app_id" ]] || no_app_record
  print_step "Validating with App Store Connect against app $app_id (no upload)"
  xcrun altool --validate-app "$pkg_path" --platform macos --apple-id "$app_id" \
    --api-key "$ASC_KEY_ID" --api-issuer "$ASC_ISSUER_ID" --p8-file-path "$ASC_KEY_PATH"
fi

print_step "App Store package is ready"
echo "PKG: $pkg_path"

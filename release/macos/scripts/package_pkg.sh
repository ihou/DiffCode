#!/usr/bin/env bash
# 把已签名、已公证的 DiffCode.app 打成安装包：root-based payload，禁止 Installer 重定位，签名、公证、装订。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

APP_NAME="${APP_NAME:-DiffCode}"
DIST_DIR="${DIST_DIR:-$PROJECT_ROOT_DIR/dist/macos}"
TEAM_ID="${TEAM_ID:-}"
DEVELOPER_ID_INSTALLER="${DEVELOPER_ID_INSTALLER:-}"
NOTARY_PROFILE="${NOTARY_PROFILE:-}"
NOTARY_KEY_PATH="${NOTARY_KEY_PATH:-}"
NOTARY_KEY_ID="${NOTARY_KEY_ID:-}"
NOTARY_ISSUER_ID="${NOTARY_ISSUER_ID:-}"
RUN_RELEASE_BUILD="${RUN_RELEASE_BUILD:-1}"
SKIP_VALIDATION="${SKIP_VALIDATION:-1}"
PKG_INSTALL_LOCATION="${PKG_INSTALL_LOCATION:-/}"
# 只在本机检查打包流程时用 0：不公证的安装包在别的 Mac 上会被 Gatekeeper 拦下。
NOTARIZE_PKG="${NOTARIZE_PKG:-1}"
[[ "$PKG_INSTALL_LOCATION" == "/" ]] || fail "PKG_INSTALL_LOCATION must be \"/\" for root-based payload packaging"

if [[ "$DIST_DIR" != /* ]]; then
  DIST_DIR="$PROJECT_ROOT_DIR/$DIST_DIR"
fi

require_command pkgbuild
require_command pkgutil
require_command productbuild
require_command shasum
require_command xcrun
require_command spctl
require_command ditto

[[ -n "$TEAM_ID" ]] || fail "TEAM_ID is required for pkg signing and notarization"

if [[ -z "$DEVELOPER_ID_INSTALLER" ]]; then
  DEVELOPER_ID_INSTALLER="$(resolve_identity "Developer ID Installer" "$TEAM_ID")"
fi
if [[ -z "$DEVELOPER_ID_INSTALLER" ]]; then
  fail "cannot find Developer ID Installer certificate for team $TEAM_ID"
fi

print_step "Using Team ID: $TEAM_ID"
print_step "Using installer identity: $DEVELOPER_ID_INSTALLER"
if [[ "$NOTARIZE_PKG" == "1" ]]; then
  has_notary_auth || fail "missing notarization auth: set NOTARY_PROFILE or all of NOTARY_KEY_PATH/NOTARY_KEY_ID/NOTARY_ISSUER_ID"
  if [[ -n "$NOTARY_PROFILE" ]]; then
    print_step "Using notary profile: $NOTARY_PROFILE"
  else
    print_step "Using notary API key credentials: $NOTARY_KEY_ID"
  fi
  print_step "Validating notary authentication"
  run_notarytool history --output-format json >/dev/null
else
  print_step "NOTARIZE_PKG=0: the app and the package are signed but not notarized"
fi

if [[ "$RUN_RELEASE_BUILD" == "1" ]]; then
  print_step "Building signed app artifact first"
  TEAM_ID="$TEAM_ID" SKIP_VALIDATION="$SKIP_VALIDATION" NOTARIZE_APP="$NOTARIZE_PKG" "$RELEASE_MACOS_DIR/scripts/build_app.sh"
fi

normalize_relative_bundle_path() {
  local value="$1"
  value="${value#./}"
  value="${value%/}"
  printf '%s' "$value"
}

verify_payload_has_main_app() {
  local pkg_file="$1"
  local label="$2"
  local payload_tmp
  local expected_payload_prefix
  local payload_path
  local found_main_app=0

  payload_tmp="$(mktemp "${TMPDIR:-/tmp}/sparkle-payload.${label}.XXXXXX")"
  expected_payload_prefix="Applications/${APP_NAME}.app"

  if ! pkgutil --payload-files "$pkg_file" > "$payload_tmp" 2>/dev/null; then
    rm -f "$payload_tmp"
    fail "failed to inspect $label payload content from: $pkg_file"
  fi

  while IFS= read -r payload_path; do
    payload_path="${payload_path#./}"
    payload_path="${payload_path#/}"
    if [[ "$payload_path" == "$expected_payload_prefix" || "$payload_path" == "$expected_payload_prefix"/* ]]; then
      found_main_app=1
      break
    fi
  done < "$payload_tmp"

  if [[ "$found_main_app" != "1" ]]; then
    local payload_head="$(head -n 5 "$payload_tmp")"
    rm -f "$payload_tmp"
    fail "expected payload path to start with $expected_payload_prefix in $label output, got: $payload_head"
  fi

  rm -f "$payload_tmp"
  print_step "Verified $label payload starts with $expected_payload_prefix"
}

app_dist_path="$DIST_DIR/$APP_NAME.app"
[[ -d "$app_dist_path" ]] || fail "app bundle not found: $app_dist_path"

version="$(parse_project_version)"
[[ -n "$version" ]] || fail "cannot parse MARKETING_VERSION from release/version.env"
bundle_id="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$app_dist_path/Contents/Info.plist" 2>/dev/null || true)"
[[ -n "$bundle_id" ]] || fail "cannot parse bundle identifier from app Info.plist"

pkg_path="$DIST_DIR/${APP_NAME}-${version}.pkg"
unsigned_pkg_path="$DIST_DIR/${APP_NAME}-${version}-unsigned.pkg"
rm -f "$pkg_path" "$unsigned_pkg_path"

stage_root="$(mktemp -d "${TMPDIR:-/tmp}/sparkle-pkg-root.XXXXXX")"
component_dir="$(mktemp -d "${TMPDIR:-/tmp}/sparkle-components.XXXXXX")"
component_plist="$component_dir/components.plist"
# pkgutil --expand refuses a destination that already exists.
expanded_pkg_parent="$(mktemp -d "${TMPDIR:-/tmp}/sparkle-expanded-pkg.XXXXXX")"
expanded_pkg="$expanded_pkg_parent/expanded"
trap 'rm -rf "$stage_root" "$component_dir" "$expanded_pkg_parent"' EXIT
mkdir -p "$stage_root/Applications"
ditto "$app_dist_path" "$stage_root/Applications/$APP_NAME.app"

print_step "Disabling Installer bundle relocation for /Applications/$APP_NAME.app"
pkgbuild --analyze --root "$stage_root" "$component_plist"
main_bundle_path="Applications/$APP_NAME.app"
normalized_main_bundle_path="$(normalize_relative_bundle_path "$main_bundle_path")"
main_bundle_index=""
component_index=0
while component_path="$(/usr/libexec/PlistBuddy -c "Print :$component_index:RootRelativeBundlePath" "$component_plist" 2>/dev/null)"; do
  normalized_component_path="$(normalize_relative_bundle_path "$component_path")"
  if [[ "$normalized_component_path" == "$normalized_main_bundle_path" ]]; then
    main_bundle_index="$component_index"
    break
  fi
  component_index=$((component_index + 1))
done
[[ -n "$main_bundle_index" ]] || fail "cannot find $main_bundle_path in pkg component plist"
/usr/libexec/PlistBuddy -c "Delete :$main_bundle_index:BundleIsRelocatable" "$component_plist" >/dev/null 2>&1 || true
/usr/libexec/PlistBuddy -c "Add :$main_bundle_index:BundleIsRelocatable bool false" "$component_plist"
bundle_relocation_flag="$(/usr/libexec/PlistBuddy -c "Print :$main_bundle_index:BundleIsRelocatable" "$component_plist" 2>/dev/null || true)"
if [[ "$bundle_relocation_flag" != "false" ]]; then
  fail "expected BundleIsRelocatable=false for $main_bundle_path, got '${bundle_relocation_flag}'"
fi
print_step "Verified component entry has BundleIsRelocatable=false: $main_bundle_path"

print_step "Building signed installer package"
pkgbuild \
  --root "$stage_root" \
  --identifier "$bundle_id" \
  --version "$version" \
  --install-location "$PKG_INSTALL_LOCATION" \
  --component-plist "$component_plist" \
  "$unsigned_pkg_path"

print_step "Inspecting unsigned package internals"
pkgutil --expand "$unsigned_pkg_path" "$expanded_pkg"
package_info="$expanded_pkg/PackageInfo"
if [[ -f "$package_info" ]]; then
  relocate_block="$(sed -n '/<relocate>/,/<\/relocate>/p' "$package_info" || true)"
  if [[ -n "$relocate_block" ]] && grep -q "bundle id=\"$bundle_id\"" <<<"$relocate_block"; then
    fail "pkg would relocate $bundle_id; expected BundleIsRelocatable=false for $main_bundle_path"
  elif [[ -z "$relocate_block" ]]; then
    print_step "Verified PackageInfo relocate block is empty (no relocation planned)"
  fi
else
  fail "cannot inspect PackageInfo after expanding unsigned pkg"
fi

verify_payload_has_main_app "$unsigned_pkg_path" "unsigned"

productbuild \
  --package "$unsigned_pkg_path" \
  --sign "$DEVELOPER_ID_INSTALLER" \
  "$pkg_path"

verify_payload_has_main_app "$pkg_path" "signed"

rm -f "$unsigned_pkg_path"

print_step "Checking package signature"
pkgutil --check-signature "$pkg_path"

if [[ "$NOTARIZE_PKG" == "1" ]]; then
  print_step "Submitting package for notarization"
  notarize_and_wait "$pkg_path"

  print_step "Stapling notarization ticket"
  xcrun stapler staple -v "$pkg_path"

  print_step "Validating installer gatekeeper status"
  spctl --assess --type install --verbose=4 "$pkg_path"
fi

# 安装包和 Sparkle 用的 zip 一起列出来，方便官网同时提供两种下载。
checksum_file="$DIST_DIR/SHA256SUMS.txt"
(
  cd "$DIST_DIR"
  checksum_targets=("$(basename "$pkg_path")")
  [[ -f "$APP_NAME.app.zip" ]] && checksum_targets+=("$APP_NAME.app.zip")
  shasum -a 256 "${checksum_targets[@]}" > "$(basename "$checksum_file")"
)

print_step "Release pkg artifact is ready"
echo "PKG: $pkg_path"
echo "SHA256: $checksum_file"

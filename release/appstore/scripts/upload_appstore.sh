#!/usr/bin/env bash
# 把 build_appstore.sh 生成的 .pkg 上传到 App Store Connect（之后在 TestFlight / 版本页里选用这个构建）。
# 只上传，不提交审核。App Store Connect 里必须已经有 bundle ID 为 com.nanshanbei.DiffCode 的 App（只能在网页上新建）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"
load_appstore_env
require_asc_key

APP_NAME="${APP_NAME:-DiffCode}"
pkg_path="${1:-$(ls -t "$DIST_DIR"/"$APP_NAME"-*-AppStore.pkg 2>/dev/null | head -n1)}"
[[ -f "$pkg_path" ]] || fail "no App Store package in $DIST_DIR; run build_appstore.sh first"

print_step "Looking up the app record for $BUNDLE_ID"
app_id="$(app_record_id)"
[[ -n "$app_id" ]] || no_app_record
print_step "App record $app_id"

print_step "Uploading $(basename "$pkg_path")"
xcrun altool --upload-package "$pkg_path" --platform macos --apple-id "$app_id" \
  --api-key "$ASC_KEY_ID" --api-issuer "$ASC_ISSUER_ID" --p8-file-path "$ASC_KEY_PATH"
print_step "Uploaded. The build shows up in App Store Connect after processing (usually 5–30 minutes); nothing was submitted for review."

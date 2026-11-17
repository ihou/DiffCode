#!/usr/bin/env bash
# 发布 Sparkle 更新到 Cloudflare R2：签名 .app.zip、生成 appcast、上传，并从公网下载回来逐项校验。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

APP_NAME="${APP_NAME:-DiffCode}"
DIST_DIR="${DIST_DIR:-$PROJECT_ROOT_DIR/dist/macos}"
R2_BUCKET="${R2_BUCKET:-}"
R2_ENDPOINT_URL="${R2_ENDPOINT_URL:-}"
R2_REGION="${R2_REGION:-auto}"
R2_PREFIX="${R2_PREFIX:-}"
R2_PUBLIC_BASE_URL="${R2_PUBLIC_BASE_URL:-https://r2-public.nanshanbei.com}"
R2_ACCESS_KEY_ID="${R2_ACCESS_KEY_ID:-}"
R2_SECRET_ACCESS_KEY="${R2_SECRET_ACCESS_KEY:-}"
R2_SESSION_TOKEN="${R2_SESSION_TOKEN:-}"
SPARKLE_SIGN_UPDATE_BIN="${SPARKLE_SIGN_UPDATE_BIN:-}"
SPARKLE_PRIVATE_KEY_FILE="${SPARKLE_PRIVATE_KEY_FILE:-}"
APPCAST_FILENAME="${APPCAST_FILENAME:-appcast.xml}"
RELEASE_NOTES_FILE="${RELEASE_NOTES_FILE:-}"
RELEASE_NOTES_URL="${RELEASE_NOTES_URL:-}"
RELEASE_NOTES_FILENAME="${RELEASE_NOTES_FILENAME:-release-notes.html}"
RELEASE_TITLE="${RELEASE_TITLE:-}"
SPARKLE_CHANNEL="${SPARKLE_CHANNEL:-}"
SPARKLE_MINIMUM_SYSTEM_VERSION="${SPARKLE_MINIMUM_SYSTEM_VERSION:-}"
SKIP_REMOTE_VERSION_CHECK="${SKIP_REMOTE_VERSION_CHECK:-0}"
DRY_RUN="${DRY_RUN:-0}"
SKIP_NOTARIZATION_CHECK="${SKIP_NOTARIZATION_CHECK:-0}"

if [[ "$DIST_DIR" != /* ]]; then
  DIST_DIR="$PROJECT_ROOT_DIR/$DIST_DIR"
fi

normalize_prefix() {
  local value="$1"
  value="${value#/}"
  value="${value%/}"
  printf '%s' "$value"
}

build_s3_key() {
  local prefix="$1"
  local file_name="$2"
  if [[ -z "$prefix" ]]; then
    printf '%s' "$file_name"
    return
  fi
  printf '%s/%s' "$prefix" "$file_name"
}

build_public_url() {
  local base_url="$1"
  local prefix="$2"
  local file_name="$3"
  local normalized_base="${base_url%/}"
  if [[ -z "$prefix" ]]; then
    printf '%s/%s' "$normalized_base" "$file_name"
    return
  fi
  printf '%s/%s/%s' "$normalized_base" "$prefix" "$file_name"
}

sanitize_file_name_part() {
  local raw="$1"
  printf '%s' "$raw" | sed -E 's/[^A-Za-z0-9._-]+/-/g; s/^-+//; s/-+$//'
}

xml_escape() {
  local raw="$1"
  raw="${raw//&/&amp;}"
  raw="${raw//</&lt;}"
  raw="${raw//>/&gt;}"
  raw="${raw//\"/&quot;}"
  raw="${raw//\'/&apos;}"
  printf '%s' "$raw"
}

resolve_sign_update_binary() {
  if [[ -n "$SPARKLE_SIGN_UPDATE_BIN" ]]; then
    printf '%s' "$SPARKLE_SIGN_UPDATE_BIN"
    return
  fi
  local found
  found="$(find_sparkle_tool sign_update || true)"
  [[ -n "$found" ]] || fail "cannot find sign_update; set SPARKLE_SIGN_UPDATE_BIN to Sparkle's sign_update path"
  printf '%s' "$found"
}

# 签名必须能被应用内置的公钥验证，否则用户装上的版本会拒绝这次更新。
verify_with_app_public_key() {
  local archive="$1"
  local signature="$2"
  local output
  if ! output="$(swift "$SCRIPT_DIR/verify_ed_signature.swift" "$app_public_key" "$signature" "$archive" 2>&1)"; then
    fail "the update signature does not verify with the app's SUPublicEDKey ($output). The signing key and the key in release/macos/AppInfo.local.env are not a pair."
  fi
}

upload_file() {
  local src_path="$1"
  local object_key="$2"
  local content_type="$3"
  local cache_control="$4"

  local destination="s3://$R2_BUCKET/$object_key"
  local command_args=(
    --endpoint-url "$R2_ENDPOINT_URL"
    s3
    cp
    "$src_path"
    "$destination"
    --content-type "$content_type"
    --region "$R2_REGION"
  )
  if [[ -n "$cache_control" ]]; then
    command_args+=(--cache-control "$cache_control")
  fi

  if [[ "$DRY_RUN" == "1" ]]; then
    echo "DRY_RUN aws ${command_args[*]}"
    return
  fi

  aws "${command_args[@]}"
}

detect_release_notes_content_type() {
  local file_path="$1"
  case "${file_path##*.}" in
    html|htm)
      printf '%s' "text/html; charset=utf-8"
      ;;
    md|markdown)
      printf '%s' "text/markdown; charset=utf-8"
      ;;
    txt)
      printf '%s' "text/plain; charset=utf-8"
      ;;
    *)
      printf '%s' "application/octet-stream"
      ;;
  esac
}

extract_first_xml_tag_value() {
  local file_path="$1"
  local tag_name="$2"
  sed -nE "s|.*<${tag_name}>([^<]+)</${tag_name}>.*|\\1|p" "$file_path" | head -n1
}

compare_version_segments() {
  local left_version="$1"
  local right_version="$2"
  local left_remaining="$left_version"
  local right_remaining="$right_version"

  while [[ -n "$left_remaining" || -n "$right_remaining" ]]; do
    local left_segment="${left_remaining%%.*}"
    local right_segment="${right_remaining%%.*}"

    if [[ "$left_remaining" == *.* ]]; then
      left_remaining="${left_remaining#*.}"
    else
      left_remaining=""
    fi

    if [[ "$right_remaining" == *.* ]]; then
      right_remaining="${right_remaining#*.}"
    else
      right_remaining=""
    fi

    [[ -n "$left_segment" ]] || left_segment="0"
    [[ -n "$right_segment" ]] || right_segment="0"

    if [[ "$left_segment" =~ ^[0-9]+$ && "$right_segment" =~ ^[0-9]+$ ]]; then
      local left_number=$((10#$left_segment))
      local right_number=$((10#$right_segment))
      if (( left_number > right_number )); then
        echo 1
        return
      fi
      if (( left_number < right_number )); then
        echo -1
        return
      fi
      continue
    fi

    if [[ "$left_segment" > "$right_segment" ]]; then
      echo 1
      return
    fi
    if [[ "$left_segment" < "$right_segment" ]]; then
      echo -1
      return
    fi
  done

  echo 0
}

verify_upgrade_version_is_newer() {
  local appcast_url="$1"
  local current_short_version="$2"
  local current_build_version="$3"
  local work_dir="$4"
  local remote_appcast_file="$work_dir/remote_appcast.xml"

  local http_code
  http_code="$(curl -sS -L -o "$remote_appcast_file" -w "%{http_code}" "$appcast_url" || true)"

  if [[ "$http_code" == "404" ]]; then
    print_step "Remote appcast not found. Treating this as first upgrade publish."
    return
  fi

  if [[ ! "$http_code" =~ ^2 ]]; then
    fail "failed to fetch remote appcast ($http_code): $appcast_url"
  fi

  local remote_short_version
  remote_short_version="$(extract_first_xml_tag_value "$remote_appcast_file" "sparkle:shortVersionString")"
  local remote_build_version
  remote_build_version="$(extract_first_xml_tag_value "$remote_appcast_file" "sparkle:version")"

  [[ -n "$remote_short_version" ]] || fail "remote appcast missing sparkle:shortVersionString"
  [[ -n "$remote_build_version" ]] || fail "remote appcast missing sparkle:version"

  local short_compare
  short_compare="$(compare_version_segments "$current_short_version" "$remote_short_version")"
  if [[ "$short_compare" == "1" ]]; then
    print_step "Version check passed: local $current_short_version > remote $remote_short_version"
    return
  fi
  if [[ "$short_compare" == "-1" ]]; then
    fail "version must increase. local short version=$current_short_version, remote short version=$remote_short_version"
  fi

  local build_compare
  build_compare="$(compare_version_segments "$current_build_version" "$remote_build_version")"
  if [[ "$build_compare" != "1" ]]; then
    fail "version must increase. local build version=$current_build_version, remote build version=$remote_build_version"
  fi

  print_step "Version check passed: short version unchanged and local build version increased ($current_build_version > $remote_build_version)"
}

validate_published_update() {
  local appcast_url="$1"
  local archive_url="$2"
  local expected_length="$3"
  local expected_signature="$4"
  local sign_update_bin="$5"
  local work_dir="$6"
  local published_appcast_file="$work_dir/published_appcast.xml"
  local published_archive_file="$work_dir/published_archive.zip"
  local http_code actual_length verify_output

  http_code="$(curl -sS -L -o "$published_appcast_file" -w "%{http_code}" "$appcast_url" || true)"
  [[ "$http_code" =~ ^2 ]] || fail "published appcast is not reachable ($http_code): $appcast_url"
  grep -Fq "$archive_url" "$published_appcast_file" || fail "published appcast does not reference archive URL: $archive_url"

  http_code="$(curl -sS -L -o "$published_archive_file" -w "%{http_code}" "$archive_url" || true)"
  [[ "$http_code" =~ ^2 ]] || fail "published archive is not reachable ($http_code): $archive_url"
  actual_length="$(stat -f%z "$published_archive_file")"
  [[ "$actual_length" == "$expected_length" ]] || fail "published archive length mismatch: expected $expected_length, got $actual_length"

  if ! verify_output="$("$sign_update_bin" --verify "$published_archive_file" "$expected_signature" 2>&1)"; then
    fail "published archive signature verification failed: $verify_output"
  fi
  verify_with_app_public_key "$published_archive_file" "$expected_signature"
}

require_command /usr/libexec/PlistBuddy
require_command stat
require_command date
require_command ditto
require_command curl
if [[ "$DRY_RUN" != "1" ]]; then
  require_command aws
fi

[[ -n "$R2_BUCKET" ]] || fail "R2_BUCKET is required"
[[ -n "$R2_ENDPOINT_URL" ]] || fail "R2_ENDPOINT_URL is required"
[[ -n "$R2_PUBLIC_BASE_URL" ]] || fail "R2_PUBLIC_BASE_URL is required"

if [[ -n "$R2_ACCESS_KEY_ID" ]]; then
  export AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID"
fi
if [[ -n "$R2_SECRET_ACCESS_KEY" ]]; then
  export AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
fi
if [[ -n "$R2_SESSION_TOKEN" ]]; then
  export AWS_SESSION_TOKEN="$R2_SESSION_TOKEN"
fi
if [[ -z "${AWS_DEFAULT_REGION:-}" ]]; then
  export AWS_DEFAULT_REGION="$R2_REGION"
fi

normalized_prefix="$(normalize_prefix "$R2_PREFIX")"
# The bucket holds other apps' feeds too: without a prefix this app's appcast.xml would land at the top and could
# replace someone else's.
[[ -n "$normalized_prefix" ]] || fail "R2_PREFIX is empty; set it to this app's own folder (e.g. diffcode/macos/stable)"

app_bundle_path="$DIST_DIR/$APP_NAME.app"
zip_path="$DIST_DIR/$APP_NAME.app.zip"
[[ -d "$app_bundle_path" ]] || fail "app bundle not found: $app_bundle_path"
if [[ ! -f "$zip_path" ]]; then
  print_step "App zip not found. Creating $zip_path"
  ditto -c -k --sequesterRsrc --keepParent "$app_bundle_path" "$zip_path"
fi

info_plist="$app_bundle_path/Contents/Info.plist"
[[ -f "$info_plist" ]] || fail "info plist not found: $info_plist"

# Each architecture has its own feed (release/macos/scripts/arch_lane.sh): an Intel app in the Apple Silicon feed,
# or the other way round, would update people to an app their Mac cannot open.
app_archs="$(lipo -archs "$app_bundle_path/Contents/MacOS/$APP_NAME" 2>/dev/null || true)"
[[ "$app_archs" == "${ARCH:-arm64}" ]] \
  || fail "$app_bundle_path is built for ${app_archs:-nothing}, but this feed is for ${ARCH:-arm64}"

app_public_key="$(/usr/libexec/PlistBuddy -c "Print :SUPublicEDKey" "$info_plist" 2>/dev/null || true)"
[[ -n "$app_public_key" ]] || fail "the app has no SUPublicEDKey; it could not verify any update"
zip_build_version="$(unzip -p "$zip_path" "$APP_NAME.app/Contents/Info.plist" 2>/dev/null | /usr/bin/plutil -extract CFBundleVersion raw -o - - 2>/dev/null || true)"
app_build_version="$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$info_plist" 2>/dev/null || true)"
[[ -n "$zip_build_version" && "$zip_build_version" == "$app_build_version" ]] \
  || fail "$zip_path does not hold the app in $app_bundle_path (build $zip_build_version vs $app_build_version); rebuild with BUILD_BEFORE_UPGRADE=1"
if [[ "$SKIP_NOTARIZATION_CHECK" != "1" ]]; then
  xcrun stapler validate "$app_bundle_path" >/dev/null 2>&1 \
    || fail "$app_bundle_path is not notarized and stapled; build it with release/macos/scripts/build_app.sh"
fi

bundle_short_version="$(/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$info_plist" 2>/dev/null || true)"
if [[ -z "$bundle_short_version" ]]; then
  bundle_short_version="$(parse_project_version)"
fi
[[ -n "$bundle_short_version" ]] || fail "cannot determine CFBundleShortVersionString"

bundle_build_version="$(/usr/libexec/PlistBuddy -c "Print :CFBundleVersion" "$info_plist" 2>/dev/null || true)"
if [[ -z "$bundle_build_version" ]]; then
  bundle_build_version="$(parse_project_build_number)"
fi
if [[ -z "$bundle_build_version" ]]; then
  bundle_build_version="$(date +%s)"
fi

archive_app_part="$(sanitize_file_name_part "$APP_NAME")"
archive_short_part="$(sanitize_file_name_part "$bundle_short_version")"
archive_build_part="$(sanitize_file_name_part "$bundle_build_version")"
[[ -n "$archive_app_part" ]] || fail "cannot build archive file name from APP_NAME=$APP_NAME"
[[ -n "$archive_short_part" ]] || fail "cannot build archive file name from short version=$bundle_short_version"
[[ -n "$archive_build_part" ]] || fail "cannot build archive file name from build version=$bundle_build_version"
archive_name="${archive_app_part}-${archive_short_part}-${archive_build_part}.app.zip"
archive_length="$(stat -f%z "$zip_path")"
archive_object_key="$(build_s3_key "$normalized_prefix" "$archive_name")"
archive_public_url="$(build_public_url "$R2_PUBLIC_BASE_URL" "$normalized_prefix" "$archive_name")"
appcast_object_key="$(build_s3_key "$normalized_prefix" "$APPCAST_FILENAME")"
appcast_public_url="$(build_public_url "$R2_PUBLIC_BASE_URL" "$normalized_prefix" "$APPCAST_FILENAME")"
# Installed copies look for updates only at the feed URL built into them.
app_feed_url="$(/usr/libexec/PlistBuddy -c "Print :SUFeedURL" "$info_plist" 2>/dev/null || true)"
[[ "$app_feed_url" == "$appcast_public_url" ]] \
  || fail "the app's SUFeedURL ($app_feed_url) is not where this appcast goes ($appcast_public_url); installed copies would never see it. Fix release/macos/AppInfo.local.env or R2_PREFIX/R2_PUBLIC_BASE_URL."

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

if [[ "$SKIP_REMOTE_VERSION_CHECK" != "1" ]]; then
  print_step "Checking remote appcast version before publish"
  verify_upgrade_version_is_newer \
    "$appcast_public_url" \
    "$bundle_short_version" \
    "$bundle_build_version" \
    "$tmp_dir"
else
  print_step "Skipping remote version check (SKIP_REMOTE_VERSION_CHECK=1)"
fi

sign_update_bin="$(resolve_sign_update_binary)"
[[ -x "$sign_update_bin" ]] || fail "sign_update binary is not executable: $sign_update_bin"

sign_args=()
if [[ -n "$SPARKLE_PRIVATE_KEY_FILE" ]]; then
  if [[ "$SPARKLE_PRIVATE_KEY_FILE" != /* ]]; then
    if [[ -n "${ENV_FILE_DIR:-}" ]]; then
      SPARKLE_PRIVATE_KEY_FILE="$ENV_FILE_DIR/$SPARKLE_PRIVATE_KEY_FILE"
    else
      SPARKLE_PRIVATE_KEY_FILE="$PROJECT_ROOT_DIR/$SPARKLE_PRIVATE_KEY_FILE"
    fi
  fi
  [[ -f "$SPARKLE_PRIVATE_KEY_FILE" ]] || fail "sparkle private key file not found: $SPARKLE_PRIVATE_KEY_FILE"
  sign_args+=(--ed-key-file "$SPARKLE_PRIVATE_KEY_FILE")
fi

print_step "Signing Sparkle update archive"
if (( ${#sign_args[@]} > 0 )); then
  signature_output="$("$sign_update_bin" "${sign_args[@]}" "$zip_path" 2>&1 || true)"
else
  signature_output="$("$sign_update_bin" "$zip_path" 2>&1 || true)"
fi
ed_signature="$(printf '%s\n' "$signature_output" | sed -nE 's/.*sparkle:edSignature="([^"]+)".*/\1/p' | head -n1)"
signed_length="$(printf '%s\n' "$signature_output" | sed -nE 's/.* length="([0-9]+)".*/\1/p' | head -n1)"
if [[ -z "$ed_signature" ]]; then
  compact_signature="$(printf '%s\n' "$signature_output" | tr -d '\r' | sed -nE '/^[A-Za-z0-9+\/=]+$/p' | head -n1)"
  if [[ -n "$compact_signature" ]]; then
    ed_signature="$compact_signature"
  fi
fi
if [[ -z "$ed_signature" ]]; then
  fail "failed to sign Sparkle archive: $signature_output"
fi
if [[ -n "$signed_length" ]]; then
  archive_length="$signed_length"
fi
verify_with_app_public_key "$zip_path" "$ed_signature"
print_step "Signature verifies with the app's SUPublicEDKey"

if [[ -n "$RELEASE_NOTES_FILE" ]]; then
  [[ -f "$RELEASE_NOTES_FILE" ]] || fail "release notes file not found: $RELEASE_NOTES_FILE"
  if [[ -z "$RELEASE_NOTES_URL" ]]; then
    notes_object_key="$(build_s3_key "$normalized_prefix" "release-notes/$RELEASE_NOTES_FILENAME")"
    notes_content_type="$(detect_release_notes_content_type "$RELEASE_NOTES_FILE")"
    print_step "Uploading release notes"
    upload_file "$RELEASE_NOTES_FILE" "$notes_object_key" "$notes_content_type" "no-cache"
    RELEASE_NOTES_URL="$(build_public_url "$R2_PUBLIC_BASE_URL" "$normalized_prefix" "release-notes/$RELEASE_NOTES_FILENAME")"
  fi
fi

if [[ -z "$RELEASE_TITLE" ]]; then
  RELEASE_TITLE="$APP_NAME $bundle_short_version"
fi

published_at="$(LC_ALL=C date -Ru)"
appcast_file="$tmp_dir/$APPCAST_FILENAME"

escaped_release_title="$(xml_escape "$RELEASE_TITLE")"
escaped_channel="$(xml_escape "$SPARKLE_CHANNEL")"
escaped_min_system="$(xml_escape "$SPARKLE_MINIMUM_SYSTEM_VERSION")"
escaped_notes_url="$(xml_escape "$RELEASE_NOTES_URL")"
escaped_archive_url="$(xml_escape "$archive_public_url")"
escaped_ed_signature="$(xml_escape "$ed_signature")"

{
  echo '<?xml version="1.0" encoding="utf-8"?>'
  echo '<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:dc="http://purl.org/dc/elements/1.1/">'
  echo '  <channel>'
  echo "    <title>$(xml_escape "$APP_NAME") Updates</title>"
  echo "    <link>$(xml_escape "$R2_PUBLIC_BASE_URL")</link>"
  echo "    <description>Latest updates for $(xml_escape "$APP_NAME")</description>"
  echo '    <language>en</language>'
  echo '    <item>'
  echo "      <title>$escaped_release_title</title>"
  echo "      <pubDate>$published_at</pubDate>"
  echo "      <sparkle:version>$bundle_build_version</sparkle:version>"
  echo "      <sparkle:shortVersionString>$bundle_short_version</sparkle:shortVersionString>"
  if [[ -n "$SPARKLE_CHANNEL" ]]; then
    echo "      <sparkle:channel>$escaped_channel</sparkle:channel>"
  fi
  if [[ -n "$SPARKLE_MINIMUM_SYSTEM_VERSION" ]]; then
    echo "      <sparkle:minimumSystemVersion>$escaped_min_system</sparkle:minimumSystemVersion>"
  fi
  if [[ -n "$RELEASE_NOTES_URL" ]]; then
    echo "      <sparkle:releaseNotesLink>$escaped_notes_url</sparkle:releaseNotesLink>"
  fi
  echo "      <enclosure url=\"$escaped_archive_url\" sparkle:edSignature=\"$escaped_ed_signature\" length=\"$archive_length\" type=\"application/octet-stream\"/>"
  echo '    </item>'
  echo '  </channel>'
  echo '</rss>'
} > "$appcast_file"

if [[ "$DRY_RUN" == "1" ]]; then
  cp "$appcast_file" "$DIST_DIR/$APPCAST_FILENAME.dry-run"
  print_step "DRY_RUN: appcast written to $DIST_DIR/$APPCAST_FILENAME.dry-run"
fi

print_step "Uploading Sparkle archive"
upload_file "$zip_path" "$archive_object_key" "application/zip" "public, max-age=31536000, immutable"
print_step "Uploading appcast"
upload_file "$appcast_file" "$appcast_object_key" "application/xml; charset=utf-8" "no-cache"
if [[ "$DRY_RUN" != "1" ]]; then
  print_step "Validating published Sparkle update"
  validate_published_update \
    "$appcast_public_url" \
    "$archive_public_url" \
    "$archive_length" \
    "$ed_signature" \
    "$sign_update_bin" \
    "$tmp_dir"
  print_step "Verified published Sparkle archive signature and length"
fi

print_step "Sparkle publish completed"
echo "Appcast URL: $appcast_public_url"
echo "Archive URL: $archive_public_url"
if [[ -n "$RELEASE_NOTES_URL" ]]; then
  echo "Release notes URL: $RELEASE_NOTES_URL"
fi

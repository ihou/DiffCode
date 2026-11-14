#!/usr/bin/env bash
# Mac App Store 版发布脚本共用的函数。

set -euo pipefail

APPSTORE_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APPSTORE_DIR="$(cd "$APPSTORE_SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT_DIR="$(cd "$APPSTORE_DIR/../.." && pwd)"

fail() {
  echo "error: $*" >&2
  exit 1
}

print_step() {
  echo "==> $*"
}

# 读 release/appstore/.appstore.env，不覆盖环境里已经设置的值；相对路径按该文件所在目录解析。
load_appstore_env() {
  local env_file="${APPSTORE_ENV_FILE:-$APPSTORE_DIR/.appstore.env}"
  [[ -f "$env_file" ]] || fail "missing $env_file (copy release/appstore/.appstore.env.example)"
  local env_dir line key
  env_dir="$(cd "$(dirname "$env_file")" && pwd)"
  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line#"${line%%[![:space:]]*}"}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    if [[ "$line" =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]; then
      key="${BASH_REMATCH[1]}"
      [[ -z "${!key+x}" ]] && export "$key=${BASH_REMATCH[2]}"
    fi
  done < "$env_file"

  TEAM_ID="${TEAM_ID:?TEAM_ID is required}"
  BUNDLE_ID="${BUNDLE_ID:-com.nanshanbei.DiffCode}"
  DIST_DIR="${DIST_DIR:-dist/appstore}"
  [[ "$DIST_DIR" == /* ]] || DIST_DIR="$PROJECT_ROOT_DIR/$DIST_DIR"
  ASC_KEY_PATH="${ASC_KEY_PATH:-}"
  [[ -z "$ASC_KEY_PATH" || "$ASC_KEY_PATH" == /* ]] || ASC_KEY_PATH="$env_dir/$ASC_KEY_PATH"
  PROVISIONING_PROFILE="${PROVISIONING_PROFILE:-profiles/DiffCode_Mac_App_Store.provisionprofile}"
  [[ "$PROVISIONING_PROFILE" == /* ]] || PROVISIONING_PROFILE="$env_dir/$PROVISIONING_PROFILE"
  export TEAM_ID BUNDLE_ID DIST_DIR ASC_KEY_PATH PROVISIONING_PROFILE
}

require_asc_key() {
  [[ -n "${ASC_KEY_ID:-}" && -n "${ASC_ISSUER_ID:-}" ]] || fail "set ASC_KEY_ID and ASC_ISSUER_ID in release/appstore/.appstore.env"
  [[ -f "$ASC_KEY_PATH" ]] || fail "App Store Connect API key not found: ${ASC_KEY_PATH:-<unset>}"
}

# App Store Connect API（release/appstore/scripts/asc.swift）。打印 JSON；HTTP 出错时返回非零。
asc() {
  ASC_KEY_ID="$ASC_KEY_ID" ASC_ISSUER_ID="$ASC_ISSUER_ID" ASC_KEY_PATH="$ASC_KEY_PATH" \
    swift "$APPSTORE_SCRIPT_DIR/asc.swift" "$@"
}

# 从 JSON（stdin）里取值，表达式里用 doc 指代整个文档：json_get 'doc["data"][0]["id"]'，可以用 len。取不到时输出空。
json_get() {
  /usr/bin/python3 -I -c '
import json, sys
try:
    value = eval(sys.argv[1], {"__builtins__": {"len": len}}, {"doc": json.load(sys.stdin)})
except Exception:
    sys.exit(0)
print(value if not isinstance(value, (dict, list)) else json.dumps(value))
' "$1"
}

# 解码后的描述文件（plist）里是否带着这张证书（按 SHA-1）。
profile_has_certificate() {
  local plist="$1" sha1="$2" index=0 encoded
  while encoded="$(plutil -extract "DeveloperCertificates.$index" raw -o - "$plist" 2>/dev/null)"; do
    [[ "$(base64 -D <<<"$encoded" | shasum -a 1 | awk '{ print toupper($1) }')" == "$sha1" ]] && return 0
    index=$((index + 1))
  done
  return 1
}

# App Store Connect 里这个 bundle ID 的 App（数字 Apple ID）；还没建 App 时输出空。App 只能在网页上新建，API 不行。
app_record_id() {
  local apps_json
  apps_json="$(asc GET "/v1/apps?filter%5BbundleId%5D=$BUNDLE_ID&fields%5Bapps%5D=bundleId")" \
    || fail "App Store Connect refused the request (see above)"
  json_get "[d[\"id\"] for d in doc[\"data\"] if d[\"attributes\"][\"bundleId\"] == \"$BUNDLE_ID\"][0]" <<<"$apps_json"
}

no_app_record() {
  fail "App Store Connect has no app with bundle ID $BUNDLE_ID yet. Create it on the web (Apps ▸ + ▸ New App: platform macOS, bundle ID $BUNDLE_ID), then run this again."
}

# 钥匙串里某类证书中有效的那一张（跳过已吊销/过期的），输出 "SHA-1<TAB>名称"。
valid_identity() {
  local kind="$1" team="$2"
  security find-identity -v -p basic \
    | grep -v "CSSMERR" \
    | sed -nE "s/^[[:space:]]*[0-9]+\) ([0-9A-F]{40}) \"(${kind}: .*\(${team}\))\"$/\1	\2/p" \
    | head -n1
}

# 证书（按 SHA-1 指定）的序列号，大写十六进制、去掉前导 0，与 App Store Connect 的 serialNumber 对得上。
certificate_serial() {
  local sha1="$1" name="$2"
  security find-certificate -a -Z -p -c "$name" \
    | awk -v want="$sha1" '/^SHA-1 hash:/ { keep = ($3 == want) } keep' \
    | sed -n '/BEGIN CERTIFICATE/,/END CERTIFICATE/p' \
    | openssl x509 -noout -serial 2>/dev/null \
    | sed -E 's/^serial=//; s/^0+//' \
    | tr '[:lower:]' '[:upper:]'
}

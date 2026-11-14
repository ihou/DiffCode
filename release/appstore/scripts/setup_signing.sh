#!/usr/bin/env bash
# Mac App Store 版的签名准备，可重复运行（已有的就跳过）。证书到期后再跑一次就会续上：
#   1. 在开发者账号里注册 bundle ID（macOS）
#   2. 钥匙串里没有有效的 Mac Installer Distribution 证书时，本机生成私钥和 CSR，向 Apple 申请证书并导入登录钥匙串
#   3. Apple Distribution 证书同上
#   4. 下载 Mac App Store 描述文件到 release/appstore/profiles/：复用对得上的；过期的、或者证书已经换掉的，删了重建
#
#   release/appstore/scripts/setup_signing.sh            执行
#   DRY_RUN=1 release/appstore/scripts/setup_signing.sh  只查询、打印将要做的事
#
# 需要账号持有人已在 App Store Connect 接受最新协议，否则 API 一律返回 403。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"
load_appstore_env
require_asc_key

DRY_RUN="${DRY_RUN:-0}"
APP_NAME="${APP_NAME:-DiffCode}"
PROFILE_NAME="${PROFILE_NAME:-$APP_NAME Mac App Store}"
keychain="$HOME/Library/Keychains/login.keychain-db"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/diffcode-asc.XXXXXX")"
chmod 700 "$work_dir"
trap 'rm -rf "$work_dir"' EXIT

# 请求体：printf 格式串加取值，写进临时文件给 asc POST。不要把 JSON 嵌在 "$(…"$(…)"…)" 里：
# bash 在那种嵌套里处理转义引号的方式会让花括号失去引号保护、被展开成好几段。
request_body="$work_dir/body.json"
write_body() {
  local format="$1"
  shift
  # shellcheck disable=SC2059
  printf "$format" "$@" > "$request_body"
}

# 1. Bundle ID
print_step "Bundle ID $BUNDLE_ID"
bundle_json="$(asc GET "/v1/bundleIds?filter%5Bidentifier%5D=$BUNDLE_ID&fields%5BbundleIds%5D=identifier,platform&limit=200")" \
  || fail "App Store Connect refused the request (see above). If it says REQUIRED_AGREEMENTS_MISSING_OR_EXPIRED, accept the agreement in App Store Connect ▸ Business first."
# The filter also matches longer identifiers; take the exact one.
bundle_resource_id="$(json_get "doc[\"data\"][[d[\"attributes\"][\"identifier\"] for d in doc[\"data\"]].index(\"$BUNDLE_ID\")][\"id\"]" <<<"$bundle_json")"
if [[ -n "$bundle_resource_id" ]]; then
  print_step "Already registered ($bundle_resource_id)"
elif [[ "$DRY_RUN" == "1" ]]; then
  print_step "DRY_RUN: would register $BUNDLE_ID for macOS as \"$APP_NAME\""
else
  write_body '{"data":{"type":"bundleIds","attributes":{"identifier":"%s","name":"%s","platform":"MAC_OS"}}}' "$BUNDLE_ID" "$APP_NAME"
  created="$(asc POST /v1/bundleIds "$request_body")"
  bundle_resource_id="$(json_get 'doc["data"]["id"]' <<<"$created")"
  [[ -n "$bundle_resource_id" ]] || fail "registering $BUNDLE_ID returned no id"
  print_step "Registered ($bundle_resource_id)"
fi

# Requests a certificate of `type` (App Store Connect's certificateType) and imports it, with its private key, into
# the login keychain. The private key never leaves this Mac: only the CSR goes to Apple. It goes into the keychain
# before the request, so that a certificate Apple issues can never end up without its key.
create_certificate() {
  local type="$1"
  local key="$work_dir/$type.key" csr="$work_dir/$type.csr" cer="$work_dir/$type.cer" csr_content created
  openssl req -new -newkey rsa:2048 -nodes -keyout "$key" -out "$csr" -subj "/CN=$APP_NAME $type/C=CN" 2>/dev/null
  # No -f: OpenSSL 3 writes PKCS#8 ("BEGIN PRIVATE KEY"), which `-f openssl` rejects as an unknown format; left to
  # itself, security reads the PEM header. -T: the tools that may sign with the key without asking each time.
  security import "$key" -k "$keychain" -t priv \
    -T /usr/bin/codesign -T /usr/bin/productbuild -T /usr/bin/productsign -T /usr/bin/pkgbuild >/dev/null \
    || fail "could not put the new private key into the login keychain; nothing was requested from Apple"
  csr_content="$(openssl req -in "$csr" -outform DER | base64 | tr -d '\n')"
  write_body '{"data":{"type":"certificates","attributes":{"certificateType":"%s","csrContent":"%s"}}}' "$type" "$csr_content"
  created="$(asc POST /v1/certificates "$request_body")"
  json_get 'doc["data"]["attributes"]["certificateContent"]' <<<"$created" | base64 -D > "$cer"
  [[ -s "$cer" ]] || fail "Apple returned no $type certificate"
  security import "$cer" -k "$keychain" >/dev/null
}

# A valid certificate of this kind in the keychain, requested when there is none (never made here, or expired).
ensure_certificate() {
  local type="$1" kind="$2" line
  print_step "$kind certificate"
  line="$(valid_identity "$kind" "$TEAM_ID")"
  if [[ -n "$line" ]]; then
    print_step "In the keychain: $(cut -f2 <<<"$line")"
  elif [[ "$DRY_RUN" == "1" ]]; then
    print_step "DRY_RUN: would create a key pair here, request a $type certificate from Apple, and import both into the login keychain"
  else
    create_certificate "$type"
    line="$(valid_identity "$kind" "$TEAM_ID")"
    [[ -n "$line" ]] || fail "the $type certificate was imported but no valid \"$kind\" identity appeared"
    print_step "Created and imported: $(cut -f2 <<<"$line")"
  fi
}

# 2. Signs the package (Mac Installer Distribution; the keychain calls it "3rd Party Mac Developer Installer").
ensure_certificate MAC_INSTALLER_DISTRIBUTION "3rd Party Mac Developer Installer"
# 3. Signs the app.
ensure_certificate DISTRIBUTION "Apple Distribution"

# 4. Mac App Store provisioning profile, for this app and the keychain's Apple Distribution certificate
print_step "Provisioning profile \"$PROFILE_NAME\""
dist_line="$(valid_identity "Apple Distribution" "$TEAM_ID")"
if [[ -z "$dist_line" ]]; then
  [[ "$DRY_RUN" == "1" ]] && { print_step "DRY_RUN: would then create a profile for the new certificate"; exit 0; }
  fail "no valid Apple Distribution certificate for team $TEAM_ID in the keychain"
fi
dist_sha1="$(cut -f1 <<<"$dist_line")"
dist_serial="$(certificate_serial "$dist_sha1" "$(cut -f2 <<<"$dist_line")")"
certs_json="$(asc GET "/v1/certificates?filter%5BcertificateType%5D=DISTRIBUTION&fields%5Bcertificates%5D=serialNumber,displayName&limit=200")"
cert_resource_id="$(json_get "doc[\"data\"][[d[\"attributes\"][\"serialNumber\"].upper().lstrip(\"0\") for d in doc[\"data\"]].index(\"$dist_serial\")][\"id\"]" <<<"$certs_json")"
[[ -n "$cert_resource_id" ]] || fail "the keychain's Apple Distribution certificate (serial $dist_serial) is not in this App Store Connect account"

mkdir -p "$(dirname "$PROVISIONING_PROFILE")"
profile_name_query="$(/usr/bin/python3 -I -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$PROFILE_NAME")"
profiles_json="$(asc GET "/v1/profiles?filter%5BprofileType%5D=MAC_APP_STORE&filter%5Bname%5D=$profile_name_query&fields%5Bprofiles%5D=name,profileState,profileContent&limit=200")"
# Reuse an active profile of this name if it is for this app and carries the keychain's certificate. The others of
# this name for this app are out of date (expired, or made for a certificate since replaced).
profile_content=""
outdated_ids=()
profile_count="$(json_get 'len(doc["data"])' <<<"$profiles_json")"
for ((i = 0; i < ${profile_count:-0}; i++)); do
  content="$(json_get "doc[\"data\"][$i][\"attributes\"][\"profileContent\"]" <<<"$profiles_json")"
  base64 -D <<<"$content" > "$work_dir/candidate.provisionprofile"
  security cms -D -i "$work_dir/candidate.provisionprofile" > "$work_dir/candidate.plist" 2>/dev/null || continue
  app_identifier="$(/usr/libexec/PlistBuddy -c "Print :Entitlements:com.apple.application-identifier" "$work_dir/candidate.plist" 2>/dev/null || true)"
  [[ "$app_identifier" == "$TEAM_ID.$BUNDLE_ID" ]] || continue
  if [[ -z "$profile_content" && "$(json_get "doc[\"data\"][$i][\"attributes\"][\"profileState\"]" <<<"$profiles_json")" == "ACTIVE" ]] \
    && profile_has_certificate "$work_dir/candidate.plist" "$dist_sha1"; then
    profile_content="$content"
  else
    outdated_ids+=("$(json_get "doc[\"data\"][$i][\"id\"]" <<<"$profiles_json")")
  fi
done

if [[ -n "$profile_content" ]]; then
  print_step "Reusing the active profile"
elif [[ "$DRY_RUN" == "1" ]]; then
  [[ "${#outdated_ids[@]}" == "0" ]] || print_step "DRY_RUN: would delete ${#outdated_ids[@]} outdated profile(s) of this name"
  print_step "DRY_RUN: would create a MAC_APP_STORE profile for $BUNDLE_ID with certificate $cert_resource_id"
  exit 0
else
  [[ -n "$bundle_resource_id" ]] || fail "no bundle ID resource to attach the profile to"
  # Out of date and in the way: a new profile may not reuse the name.
  for id in ${outdated_ids[@]+"${outdated_ids[@]}"}; do
    asc DELETE "/v1/profiles/$id" >/dev/null
    print_step "Deleted the outdated profile $id"
  done
  write_body '{"data":{"type":"profiles","attributes":{"name":"%s","profileType":"MAC_APP_STORE"},"relationships":{"bundleId":{"data":{"type":"bundleIds","id":"%s"}},"certificates":{"data":[{"type":"certificates","id":"%s"}]}}}}' \
    "$PROFILE_NAME" "$bundle_resource_id" "$cert_resource_id"
  created="$(asc POST /v1/profiles "$request_body")"
  profile_content="$(json_get 'doc["data"]["attributes"]["profileContent"]' <<<"$created")"
  [[ -n "$profile_content" ]] || fail "creating the profile returned no content"
  print_step "Created"
fi
base64 -D <<<"$profile_content" > "$PROVISIONING_PROFILE"
security cms -D -i "$PROVISIONING_PROFILE" >/dev/null 2>&1 || fail "the downloaded profile does not decode"
print_step "Profile saved to $PROVISIONING_PROFILE"

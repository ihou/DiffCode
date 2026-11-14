#!/usr/bin/env bash
# 官网下载版发布脚本共用的函数。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RELEASE_MACOS_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"

fail() {
  echo "error: $*" >&2
  exit 1
}

print_step() {
  echo "==> $*"
}

require_command() {
  local command_name="$1"
  if ! command -v "$command_name" >/dev/null 2>&1; then
    fail "missing required command: $command_name"
  fi
}

resolve_identity() {
  local identity_type="$1"
  local team_id="$2"

  [[ -n "$team_id" ]] || return 0

  security find-identity -v -p basic \
    | sed -nE "s/^[[:space:]]*[0-9]+\) [0-9A-F]+ \"(${identity_type}: .*\(${team_id}\))\"(.*)$/\1\t\2/p" \
    | grep -v "CSSMERR" \
    | head -n1 \
    | cut -f1 \
    || true
}

# 版本号统一来自 release/version.env（两个渠道共用）。
parse_project_version() {
  sed -nE 's/^MARKETING_VERSION=([^[:space:]]+).*/\1/p' "$PROJECT_ROOT_DIR/release/version.env" | head -n1
}

parse_project_build_number() {
  sed -nE 's/^BUILD_NUMBER=([0-9]+).*/\1/p' "$PROJECT_ROOT_DIR/release/version.env" | head -n1
}

# Sparkle 的命令行工具：先找 PATH，再用 SwiftPM 解析依赖时下载的那一份（与应用内的 Sparkle 同版本）。
find_sparkle_tool() {
  local tool="$1"
  if command -v "$tool" >/dev/null 2>&1; then
    command -v "$tool"
    return
  fi
  local bundled="$PROJECT_ROOT_DIR/.build/artifacts/sparkle/Sparkle/bin/$tool"
  if [[ ! -x "$bundled" ]]; then
    (cd "$PROJECT_ROOT_DIR" && swift package resolve >/dev/null)
  fi
  [[ -x "$bundled" ]] && printf '%s' "$bundled"
}

run_notarytool() {
  if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    xcrun notarytool "$@" --keychain-profile "$NOTARY_PROFILE" --team-id "$TEAM_ID"
    return
  fi
  [[ -n "${NOTARY_KEY_PATH:-}" && -n "${NOTARY_KEY_ID:-}" && -n "${NOTARY_ISSUER_ID:-}" ]] \
    || fail "missing notarization auth: set NOTARY_PROFILE or all of NOTARY_KEY_PATH/NOTARY_KEY_ID/NOTARY_ISSUER_ID"
  [[ -f "$NOTARY_KEY_PATH" ]] || fail "notary key file not found: $NOTARY_KEY_PATH"
  xcrun notarytool "$@" --key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" --team-id "$TEAM_ID"
}

# 提交公证并等待结果。notarytool 在结果为 Invalid 时也可能正常退出，所以按 JSON 里的 status 判断，失败时打印 Apple 的日志。
notarize_and_wait() {
  local file="$1"
  local submission status submission_id
  submission="$(run_notarytool submit "$file" --wait --output-format json)" || fail "notarization submission failed for $file"
  status="$(/usr/bin/plutil -extract status raw -o - - <<<"$submission" 2>/dev/null || true)"
  submission_id="$(/usr/bin/plutil -extract id raw -o - - <<<"$submission" 2>/dev/null || true)"
  if [[ "$status" != "Accepted" ]]; then
    [[ -n "$submission_id" ]] && run_notarytool log "$submission_id" >&2 || true
    fail "notarization of $(basename "$file") finished with status '${status:-unknown}'"
  fi
  print_step "Notarization accepted ($submission_id)"
}

has_notary_auth() {
  [[ -n "${NOTARY_PROFILE:-}" ]] || [[ -n "${NOTARY_KEY_PATH:-}" && -f "${NOTARY_KEY_PATH:-}" && -n "${NOTARY_KEY_ID:-}" && -n "${NOTARY_ISSUER_ID:-}" ]]
}

#!/usr/bin/env bash
# 官网下载版：构建 Developer ID 签名、公证、装订好的 DiffCode.app / DiffCode.app.zip，再打成同样公证过的 .pkg 安装包。
# 配置来自 release/macos/.macos.env（见 .macos.env.example），版本号来自 release/version.env。
#
#   ./build_macos.sh                         完整构建：app + zip + pkg
#   RUN_RELEASE_BUILD=0 ./build_macos.sh     只用 dist/macos 里现成的 app 重新打 pkg
#   ARCH=x86_64 ./build_macos.sh             Intel 版，放在 dist/macos-intel（见 release/macos/scripts/arch_lane.sh）

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_ENV_FILE="$ROOT_DIR/release/macos/.macos.env"
if [[ ! -f "$DEFAULT_ENV_FILE" && -f "$ROOT_DIR/.macos.env" ]]; then
  DEFAULT_ENV_FILE="$ROOT_DIR/.macos.env"
fi
ENV_FILE="${ENV_FILE:-$DEFAULT_ENV_FILE}"
if [[ "$ENV_FILE" != /* ]]; then
  ENV_FILE="$ROOT_DIR/$ENV_FILE"
fi
ENV_FILE_DIR="$(cd "$(dirname "$ENV_FILE")" && pwd)"

load_env_defaults() {
  local env_file="$1"
  [[ -f "$env_file" ]] || return 0

  local raw_line line key
  while IFS= read -r raw_line || [[ -n "$raw_line" ]]; do
    line="${raw_line#"${raw_line%%[![:space:]]*}"}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    if [[ "$line" =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]; then
      key="${BASH_REMATCH[1]}"
      if [[ -z "${!key+x}" ]]; then
        export "$line"
      fi
    fi
  done < "$env_file"
}

load_env_defaults "$ENV_FILE"
# shellcheck disable=SC1091
source "$ROOT_DIR/release/macos/scripts/arch_lane.sh"

TEAM_ID="${TEAM_ID:-}"
PKG_INSTALL_LOCATION="${PKG_INSTALL_LOCATION:-/}"
NOTARY_KEY_ID="${NOTARY_KEY_ID:-}"
NOTARY_ISSUER_ID="${NOTARY_ISSUER_ID:-}"
NOTARY_KEY_PATH="${NOTARY_KEY_PATH:-}"
SKIP_VALIDATION="${SKIP_VALIDATION:-1}"
RUN_RELEASE_BUILD="${RUN_RELEASE_BUILD:-1}"
# A .p8 kept outside the repository (for example one shared by several apps). It is used when NOTARY_KEY_PATH
# does not exist and names the same key file.
SHARED_NOTARY_KEY_PATH="${SHARED_NOTARY_KEY_PATH:-}"
if [[ -n "$NOTARY_KEY_PATH" && "$NOTARY_KEY_PATH" != /* ]]; then
  NOTARY_KEY_PATH="$ENV_FILE_DIR/$NOTARY_KEY_PATH"
fi

cd "$ROOT_DIR"

if [[ -z "${NOTARY_PROFILE:-}" && -n "$NOTARY_KEY_ID" && -z "$NOTARY_KEY_PATH" ]]; then
  NOTARY_KEY_PATH="$ENV_FILE_DIR/AuthKey_${NOTARY_KEY_ID}.p8"
fi

if [[ -z "${NOTARY_PROFILE:-}" && -n "$NOTARY_KEY_PATH" && ! -f "$NOTARY_KEY_PATH" \
      && -n "$SHARED_NOTARY_KEY_PATH" && -f "$SHARED_NOTARY_KEY_PATH" \
      && "$(basename "$NOTARY_KEY_PATH")" == "$(basename "$SHARED_NOTARY_KEY_PATH")" ]]; then
  NOTARY_KEY_PATH="$SHARED_NOTARY_KEY_PATH"
fi

if [[ -z "${NOTARY_PROFILE:-}" && -z "$NOTARY_KEY_PATH" ]]; then
  echo "error: missing notarization auth. Set NOTARY_PROFILE or NOTARY_KEY_PATH in $ENV_FILE" >&2
  exit 1
fi

if [[ -z "${NOTARY_PROFILE:-}" && ! -f "$NOTARY_KEY_PATH" ]]; then
  echo "error: notary key file not found: $NOTARY_KEY_PATH" >&2
  echo "hint: put AuthKey_${NOTARY_KEY_ID}.p8 next to $ENV_FILE, set NOTARY_PROFILE, or set SHARED_NOTARY_KEY_PATH" >&2
  exit 1
fi

# 默认用 release/macos/.macos.env 旁边的 .p8；没有时用 SHARED_NOTARY_KEY_PATH 指向的仓库外的那一份（不复制进仓库）。
echo "==> Build macOS package"
if [[ -n "$TEAM_ID" ]]; then
  echo "==> Team ID: $TEAM_ID"
fi
if [[ -n "${NOTARY_PROFILE:-}" ]]; then
  echo "==> Notary Profile: $NOTARY_PROFILE"
else
  echo "==> Notary Key ID: $NOTARY_KEY_ID"
  echo "==> Notary Key Path: $NOTARY_KEY_PATH"
fi
echo "==> Package Install Location: $PKG_INSTALL_LOCATION"
if [[ "$PKG_INSTALL_LOCATION" != "/" ]]; then
  echo "error: PKG_INSTALL_LOCATION must be \"/\" for root-based payload packaging" >&2
  exit 1
fi

export ENV_FILE_DIR

TEAM_ID="$TEAM_ID" \
NOTARY_KEY_PATH="$NOTARY_KEY_PATH" \
NOTARY_KEY_ID="$NOTARY_KEY_ID" \
NOTARY_ISSUER_ID="$NOTARY_ISSUER_ID" \
SKIP_VALIDATION="$SKIP_VALIDATION" \
RUN_RELEASE_BUILD="$RUN_RELEASE_BUILD" \
PKG_INSTALL_LOCATION="$PKG_INSTALL_LOCATION" \
NOTARY_PROFILE="${NOTARY_PROFILE:-}" \
exec "$ROOT_DIR/release/macos/scripts/package_pkg.sh" "$@"

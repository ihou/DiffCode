#!/usr/bin/env bash
# 官网下载版：把新版本作为 Sparkle 更新发布到 Cloudflare R2（appcast.xml + 带版本号的 .app.zip），并从公网回读校验。
# 先在 release/version.env 里把 MARKETING_VERSION / BUILD_NUMBER 调大；远端已有同版本或更高版本时直接失败。
#
#   ./upgrade_macos.sh                           构建（含公证）后发布
#   BUILD_BEFORE_UPGRADE=0 ./upgrade_macos.sh    发布 dist/macos 里现成的 app
#   DRY_RUN=1 ./upgrade_macos.sh                 只签名、生成 appcast，不上传
#   ARCH=x86_64 ./upgrade_macos.sh               Intel 版，发布到它自己的更新源（见 release/macos/scripts/arch_lane.sh）

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

cd "$ROOT_DIR"
export ENV_FILE_DIR

# 与 build_macos.sh 相同：相对路径的 .p8 放在 .macos.env 旁边，没有时用 SHARED_NOTARY_KEY_PATH 指向的仓库外的那一份。
NOTARY_KEY_ID="${NOTARY_KEY_ID:-}"
NOTARY_KEY_PATH="${NOTARY_KEY_PATH:-}"
SHARED_NOTARY_KEY_PATH="${SHARED_NOTARY_KEY_PATH:-}"
if [[ -n "$NOTARY_KEY_PATH" && "$NOTARY_KEY_PATH" != /* ]]; then
  NOTARY_KEY_PATH="$ENV_FILE_DIR/$NOTARY_KEY_PATH"
fi
if [[ -z "${NOTARY_PROFILE:-}" && -n "$NOTARY_KEY_PATH" && ! -f "$NOTARY_KEY_PATH" \
      && -n "$SHARED_NOTARY_KEY_PATH" && -f "$SHARED_NOTARY_KEY_PATH" \
      && "$(basename "$NOTARY_KEY_PATH")" == "$(basename "$SHARED_NOTARY_KEY_PATH")" ]]; then
  NOTARY_KEY_PATH="$SHARED_NOTARY_KEY_PATH"
fi
export NOTARY_KEY_PATH

BUILD_BEFORE_UPGRADE="${BUILD_BEFORE_UPGRADE:-1}"

if [[ "$BUILD_BEFORE_UPGRADE" == "1" ]]; then
  echo "==> Build latest app artifact before upgrade publish"
  "$ROOT_DIR/release/macos/scripts/build_app.sh"
fi

echo "==> Publish Sparkle upgrade"
exec "$ROOT_DIR/release/macos/scripts/publish_sparkle_r2.sh" "$@"

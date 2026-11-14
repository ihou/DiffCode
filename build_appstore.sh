#!/usr/bin/env bash
# Mac App Store 版：构建、签名、打包并让 App Store Connect 预检（不上传）。说明见 release/appstore/README.md。
#
#   release/appstore/scripts/setup_signing.sh   第一次：注册 bundle ID、申请安装包证书、下载描述文件
#   ./build_appstore.sh                         构建 dist/appstore/DiffCode-<版本>-<build>-AppStore.pkg 并预检
#   ./upload_appstore.sh                        上传到 App Store Connect（不提交审核）
set -euo pipefail
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release/appstore/scripts/build_appstore.sh" "$@"

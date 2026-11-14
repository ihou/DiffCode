#!/usr/bin/env bash
# Mac App Store 版：上传 build_appstore.sh 生成的安装包。只上传，不提交审核。说明见 release/appstore/README.md。
set -euo pipefail
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release/appstore/scripts/upload_appstore.sh" "$@"

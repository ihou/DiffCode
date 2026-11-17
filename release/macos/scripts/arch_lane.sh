#!/usr/bin/env bash
# build_macos.sh 和 upgrade_macos.sh 在读完 .macos.env 之后 source 这个文件。
#
# 默认（ARCH=arm64）是 Apple Silicon 版。Intel 版（ARCH=x86_64）是单独的下载、单独的 Sparkle 更新源，
# 这样 Apple Silicon 版保持小巧：产物放 dist/macos-intel（可用 DIST_DIR_INTEL 改），R2 前缀是 R2_PREFIX 后面
# 加 "-intel"（可用 R2_PREFIX_INTEL 改），应用的 SUFeedURL 指向那个前缀下的 appcast。
# Intel 版绝不能指向 Apple Silicon 的更新源，否则 Intel 用户会被"更新"成打不开的版本，所以算不出来就直接失败。

ARCH="${ARCH:-arm64}"
case "$ARCH" in
  arm64) ;;
  x86_64)
    DIST_DIR="${DIST_DIR_INTEL:-dist/macos-intel}"
    if [[ -n "${R2_PREFIX_INTEL:-}" ]]; then
      R2_PREFIX="$R2_PREFIX_INTEL"
    elif [[ -n "${R2_PREFIX:-}" ]]; then
      R2_PREFIX="${R2_PREFIX%/}-intel"
    else
      echo "error: the Intel build needs R2_PREFIX (or R2_PREFIX_INTEL) for its own update feed" >&2
      exit 1
    fi
    if [[ -z "${R2_PUBLIC_BASE_URL:-}" ]]; then
      echo "error: the Intel build needs R2_PUBLIC_BASE_URL for its own update feed" >&2
      exit 1
    fi
    SPARKLE_FEED_URL="${R2_PUBLIC_BASE_URL%/}/$R2_PREFIX/${APPCAST_FILENAME:-appcast.xml}"
    export DIST_DIR R2_PREFIX SPARKLE_FEED_URL
    echo "==> Intel build: $DIST_DIR, update feed under $R2_PREFIX"
    ;;
  *)
    echo "error: ARCH must be arm64 or x86_64" >&2
    exit 1
    ;;
esac
export ARCH

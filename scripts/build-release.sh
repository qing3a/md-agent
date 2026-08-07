#!/usr/bin/env bash
# md-agent 自动构建测试包：检测代码改动 → cargo build --release → 部署 dist-test/（完整可运行，不碰正式 dist/）
# 用法: bash scripts/build-release.sh [--force]
# dist-test/ = exe + web/ + config.json + kb 快照（首次复制）；exe 被运行中实例占用时不覆盖、保留旧 stamp 下次重试
set -e
cd "$(dirname "$0")/.."

STAMP="dist-test/.build-stamp"
SRC_DIRS="src web"
FILES=(Cargo.toml)

fingerprint() {
  local head files
  head=$(git rev-parse HEAD 2>/dev/null || echo "no-git")
  files=$(find $SRC_DIRS -type f 2>/dev/null | sort)
  printf '%s' "$head" | md5sum | cut -d' ' -f1
  # 文件 mtime 集合哈希（覆盖未提交的本地改动）
  for f in $files "${FILES[@]}"; do
    if [ -f "$f" ]; then stat -c '%Y %n' "$f"; fi
  done | md5sum | cut -d' ' -f1
}

NEW=$(fingerprint | tr '\n' ':')
OLD=""
[ -f "$STAMP" ] && OLD=$(cat "$STAMP" 2>/dev/null || echo "")

if [ "$1" != "--force" ] && [ -n "$OLD" ] && [ "$NEW" = "$OLD" ]; then
  echo "无代码改动，跳过构建（指纹一致）"
  exit 0
fi

echo "检测到代码改动，构建 release..."
if ! cargo build --release 2>&1 | tail -4; then
  mkdir -p dist-test
  echo "构建失败 $(date '+%Y-%m-%d %H:%M:%S')" > dist-test/.build-error.log
  echo "❌ 构建失败，详见 dist-test/.build-error.log"
  exit 1
fi

mkdir -p dist-test dist-test/web
# exe：被占用则不动（保留旧 stamp → 下次检测重试）
if cp target/release/md-agent.exe dist-test/md-agent.exe 2>/dev/null; then
  EXE_OK=1
else
  EXE_OK=0
  echo "⚠ dist-test/md-agent.exe 被运行中实例占用，新 exe 未覆盖（保留旧 stamp，下次检测重试）"
fi
# web 增量同步（不删目录，避免锁定运行中的测试实例）
cp -r web/. dist-test/web/
# config / kb：仅首次复制（用户测试中的数据改动不被覆盖）
[ -f dist-test/config.json ] || cp dist/config.json dist-test/config.json 2>/dev/null || true
[ -d dist-test/kb ] || cp -r dist/kb dist-test/kb 2>/dev/null || true

if [ "$EXE_OK" = "1" ]; then
  echo "$NEW" > "$STAMP"
  echo "✓ 构建并部署完成: dist-test/md-agent.exe  $(date '+%H:%M:%S')"
  echo "启动测试: cd dist-test && ./md-agent.exe --no-tray --port 8901  （托盘模式直接 ./md-agent.exe）"
else
  echo "exe 未更新（占用），web 已同步；下次检测将重试 exe"
  exit 2
fi

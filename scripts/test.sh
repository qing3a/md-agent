#!/usr/bin/env bash
# md-agent 一键回归：Rust 单测 + 前端逻辑测试 + 端到端四型审批链路
# 用法: bash scripts/test.sh [--skip-build]
set -e
cd "$(dirname "$0")/.."

echo "=== 1/3 Rust 单测 ==="
if [ "$1" != "--skip-build" ]; then
  cargo build 2>&1 | tail -1
fi
cargo test 2>&1 | grep -E "test result" | tail -1

echo ""
echo "=== 2/3 前端逻辑测试（core.js）==="
node --check web/app.js
node scripts/frontend-test.js | tail -2

echo ""
echo "=== 3/3 端到端四型审批链路（隔离 kb）==="
python scripts/e2e.py

echo ""
echo "全部完成：exit=$?"

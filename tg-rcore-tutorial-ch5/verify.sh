#!/usr/bin/env bash
# 实验 10 · ch5 多核验收：进程管理 / Stride 调度仍在 hart 0；副核仅上线后 wfi。
# 用法（必须在 ch5 目录）: ./verify.sh
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n\033[1;33m%s\033[0m\n' "$*"; }

say "════════════════════════════════════════════════════════════"
say "  ch5 多核验收 — initproc / shell 仅在 hart 0"
say "════════════════════════════════════════════════════════════"
echo ""
echo "【多核 8 核】请看输出最前面："
echo "  1) [qemu-virt-kernel] RCORE_SMP=8"
echo "  2) 连续 7 行：[ch5] hart … secondary (scheduler on hart 0 only)"
echo "  3) 然后才是 LOG / initproc 等"
echo ""
echo "（ch5 含 shell 与用户程序，整次运行可能很长；可按 Ctrl+A X 退出 QEMU。）"
echo ""

if [[ -z "${SMPTEST_QUICK:-}" ]] && [[ -t 0 ]]; then
  read -r -p "按回车开始多核运行… "
fi

say "—— 多核运行开始 ——"
RCORE_SMP=8 RCORE_QEMU_VERBOSE=1 cargo run --release
say "—— 多核运行结束 ——"

echo ""
say "【单核对照】RCORE_SMP=1（开头无 7 行副核）"
echo ""

if [[ -z "${SMPTEST_QUICK:-}" ]] && [[ -t 0 ]]; then
  read -r -p "按回车开始单核运行… "
fi

say "—— 单核运行开始 ——"
RCORE_SMP=1 RCORE_QEMU_VERBOSE=1 cargo run --release
say "—— 单核运行结束 ——"

echo ""
echo "验收要点：多核时开头多 7 行 [ch5] secondary；单核时没有。"

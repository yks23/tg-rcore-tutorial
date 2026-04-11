#!/usr/bin/env bash
# 实验 10 · ch3 多核验收：调度与时钟中断仍在 hart 0；副核仅上线后 wfi。
# 用法（必须在 ch3 目录）: ./verify.sh
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n\033[1;33m%s\033[0m\n' "$*"; }

say "════════════════════════════════════════════════════════════"
say "  ch3 多核验收 — 抢占/轮转调度仅在 hart 0"
say "════════════════════════════════════════════════════════════"
echo ""
echo "【多核 8 核】请看输出最前面："
echo "  1) [qemu-virt-kernel] RCORE_SMP=8"
echo "  2) 连续 7 行：[ch3] hart … secondary (scheduler on hart 0 only)"
echo "  3) 然后才是 [TRACE] LOG TEST、load app…"
echo "  若 2) 在 3) 之后，说明 BSS_READY / 初始化顺序有问题。"
echo ""
echo "（跑完全部用户任务后关机，可能稍久。）"
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
echo "验收要点：多核时开头多 7 行 [ch3] secondary；单核时没有。"

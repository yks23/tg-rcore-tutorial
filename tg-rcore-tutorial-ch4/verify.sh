#!/usr/bin/env bash
# 实验 10 · ch4 多核验收：Sv39 + 调度线程仍在 hart 0；副核仅上线后 wfi。
# 用法（必须在 ch4 目录）: ./verify.sh
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n\033[1;33m%s\033[0m\n' "$*"; }

say "════════════════════════════════════════════════════════════"
say "  ch4 多核验收 — 异界传送门 / 调度仅在 hart 0"
say "════════════════════════════════════════════════════════════"
echo ""
echo "【多核 8 核】请看输出最前面："
echo "  1) [qemu-virt-kernel] RCORE_SMP=8"
echo "  2) 连续 7 行：[ch4] hart … secondary (scheduler on hart 0 only)"
echo "  3) 然后才是 ASCII 横幅、LOG TEST、detect app…"
echo ""
echo "（ch4 会跑完所有 ELF 用户进程，时间较长。）"
echo ""

if [[ -z "${SMPTEST_QUICK:-}" ]] && [[ -t 0 ]]; then
  read -r -p "按回车开始多核全量运行… "
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
echo "验收要点：多核时开头多 7 行 [ch4] secondary；单核时没有。"

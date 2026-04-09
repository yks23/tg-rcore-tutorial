#!/usr/bin/env bash
# 实验 9 · ch2 多核：终端直接看输出；批处理较长，多核现象在开头最明显。
# 用法（必须在 ch2 目录）:
#   ./test-smp.sh
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n\033[1;33m%s\033[0m\n' "$*"; }

say "════════════════════════════════════════════════════════════"
say "  ch2 多核验收 — 应用仍只在 hart 0 跑"
say "════════════════════════════════════════════════════════════"
echo ""
echo "【多核 8 核】请盯着输出最前面："
echo "  1) [qemu-virt-kernel] RCORE_SMP=8"
echo "  2) 连续 7 行：[ch2] hart … secondary (batch on hart 0 only)"
echo "  3) 然后才是 ASCII 横幅、[TRACE] LOG TEST、load app0 …"
echo "  若 2) 和 3) 顺序反了，说明 BSS/初始化顺序有问题。"
echo ""
echo "（整次跑完要一会儿，会跑完所有用户程序后关机。）"
echo ""

if [[ -z "${SMPTEST_QUICK:-}" ]] && [[ -t 0 ]]; then
  read -r -p "按回车开始多核全量运行… "
fi

say "—— 多核运行开始 ——"
RCORE_SMP=8 RCORE_QEMU_VERBOSE=1 cargo run --release
say "—— 多核运行结束 ——"

echo ""
say "════════════════════════════════════════════════════════════"
say "【单核对照】RCORE_SMP=1（仍会跑完全部 app，但开头无 7 行副核）"
echo ""

if [[ -z "${SMPTEST_QUICK:-}" ]] && [[ -t 0 ]]; then
  read -r -p "按回车开始单核运行… "
fi

say "—— 单核运行开始 ——"
RCORE_SMP=1 RCORE_QEMU_VERBOSE=1 cargo run --release
say "—— 单核运行结束 ——"

echo ""
echo "验收要点：多核时开头多 7 行副核提示；单核时没有。批处理日志应一致。"

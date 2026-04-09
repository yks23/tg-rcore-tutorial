#!/usr/bin/env bash
# 实验 9 · ch1 多核：在终端直接看串口输出（不要再用管道 grep 抓日志）。
# 用法（必须在 ch1 目录）:
#   ./test-smp.sh
# 或:
#   bash test-smp.sh
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\n\033[1;33m%s\033[0m\n' "$*"; }

say "════════════════════════════════════════════════════════════"
say "  ch1 多核验收 — 现象会直接打印在下面（QEMU -nographic）"
say "════════════════════════════════════════════════════════════"
echo ""
echo "【先看多核】默认 RCORE_SMP=8"
echo "  你应看到："
echo "    • 一行 [qemu-virt-kernel] RCORE_SMP=8 ..."
echo "    • 连续 7 行：[ch1] hart 1..7 secondary online（顺序可乱）"
echo "    • 最后一行：Hello, world!"
echo "  然后 QEMU 退出。"
echo ""

if [[ -z "${SMPTEST_QUICK:-}" ]] && [[ -t 0 ]]; then
  read -r -p "准备好就按回车开始多核运行… "
fi

say "—— 多核运行开始 ——"
RCORE_SMP=8 RCORE_QEMU_VERBOSE=1 cargo run --release
say "—— 多核运行结束 ——"

echo ""
say "════════════════════════════════════════════════════════════"
say "【再看单核对照】RCORE_SMP=1"
echo "  你应看到："
echo "    • [qemu-virt-kernel] RCORE_SMP=1"
echo "    • 没有 [ch1] hart … secondary 行"
echo "    • 仍有 Hello, world!"
echo "  用来证明：副核行是 SMP 带来的，不是写死多打几行。"
echo ""

if [[ -z "${SMPTEST_QUICK:-}" ]] && [[ -t 0 ]]; then
  read -r -p "按回车开始单核运行… "
fi

say "—— 单核运行开始 ——"
RCORE_SMP=1 RCORE_QEMU_VERBOSE=1 cargo run --release
say "—— 单核运行结束 ——"

echo ""
echo "验收完成。若多核段没有 7 行副核信息，检查："
echo "  • 是否在 tg-rcore-tutorial-ch1 目录执行"
echo "  • main.rs 是否已含实验 9 SMP 逻辑"
echo "  • 不要用 export RCORE_SMP=1 占满整个 shell 后忘了 unset"

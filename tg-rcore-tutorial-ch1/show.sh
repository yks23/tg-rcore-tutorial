#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL="$ROOT/target/riscv64gc-unknown-none-elf/debug/tg-rcore-tutorial-ch1-yks23-tangram"
SCREENSHOT="$ROOT/screenshot-tangram.ppm"
MON_PORT=44401

usage() {
  echo "用法: $0 [default|vnc|gui]"
  echo "  default  无头 + monitor 截图（终端预览）"
  echo "  vnc      VNC :1 → 本机连 127.0.0.1:5901（或 SSH -L 转发后连本地）"
  echo "  gui      本机有图形界面时弹出 QEMU 窗口（需 DISPLAY，如 :0）"
}

mode="${1:-default}"

case "$mode" in
  vnc)
    echo "=== ch1: VNC（127.0.0.1:5901）==="
    RCORE_SMP=1 cargo build 2>&1 | tail -1
    if [[ ! -f "$KERNEL" ]]; then
      echo "缺少内核: $KERNEL（请先 cargo build）" >&2
      exit 1
    fi
    exec qemu-system-riscv64 \
      -machine virt -smp 1 -bios none \
      -device virtio-gpu-device \
      -vnc :1 \
      -serial stdio \
      -kernel "$KERNEL"
    ;;
  gui)
    echo "=== ch1: 本地图形窗口（GTK）==="
    if [[ -z "${DISPLAY:-}" ]]; then
      echo "未设置 DISPLAY，无法打开图形窗口。请登录图形桌面或执行: export DISPLAY=:0" >&2
      echo "远程可用: $0 vnc" >&2
      exit 1
    fi
    RCORE_SMP=1 cargo build 2>&1 | tail -1
    if [[ ! -f "$KERNEL" ]]; then
      echo "缺少内核: $KERNEL" >&2
      exit 1
    fi
    exec qemu-system-riscv64 \
      -machine virt -smp 1 -bios none \
      -device virtio-gpu-device \
      -display gtk \
      -serial stdio \
      -kernel "$KERNEL"
    ;;
  help|-h|--help)
    usage
    ;;
  default|*)
    echo "=== ch1-tangram: 七巧板 OS 图案 (VirtIO-GPU) ==="
    usage
    echo "编译内核..."
    RCORE_SMP=1 cargo build 2>&1 | tail -1

    rm -f "$SCREENSHOT"

    echo "启动 QEMU (VirtIO-GPU + monitor)..."
    qemu-system-riscv64 \
      -machine virt -smp 1 -bios none \
      -device virtio-gpu-device \
      -display none \
      -serial stdio \
      -monitor "telnet:127.0.0.1:$MON_PORT,server,nowait" \
      -kernel "$KERNEL" &
    QEMU_PID=$!

    sleep 6

    for _ in $(seq 1 15); do
      if kill -0 "$QEMU_PID" 2>/dev/null; then
        (echo "screendump $SCREENSHOT"; sleep 0.5) | nc -q 1 127.0.0.1 "$MON_PORT" 2>/dev/null && break || true
        sleep 1
      else
        break
      fi
    done

    sleep 2
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true

    if [ -f "$SCREENSHOT" ]; then
      echo ""
      echo "=== GPU 截图已保存: $SCREENSHOT ==="
      python3 -c "
from PIL import Image
img = Image.open('$SCREENSHOT')
w, h = img.size
tw, th = 80, 40
img2 = img.resize((tw, th))
for y in range(th):
    line = ''
    for x in range(tw):
        r, g, b = img2.getpixel((x, y))[:3]
        line += f'\033[48;2;{r};{g};{b}m  '
    line += '\033[0m'
    print(line)
print(f'\n原始分辨率: {w}x{h}')
" 2>/dev/null || echo "(pip install Pillow 可终端预览)"
    else
      echo "警告: 截图未生成（QEMU 可能在截图前退出）"
    fi
    ;;
esac

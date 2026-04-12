#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FS_IMG="$ROOT/target/riscv64gc-unknown-none-elf/debug/fs.img"
KERNEL="$ROOT/target/riscv64gc-unknown-none-elf/debug/tg-rcore-tutorial-ch8-yks23"
SCREENSHOT="$ROOT/screenshot-doom.ppm"
MON_PORT=44408

usage() {
  echo "用法: $0 [default|vnc|gui|demo]"
  echo "  default  无头 + monitor 截图"
  echo "  vnc      VNC :1 → 127.0.0.1:5901（SSH: ssh -L 5901:127.0.0.1:5901 user@host）"
  echo "  gui      本机有图形界面时弹出 QEMU 窗口（需 DISPLAY）"
  echo "  demo     CHAPTER=game → doom + 自动截图"
}

mode="${1:-default}"

case "$mode" in
  vnc)
    echo "=== ch8: VNC（127.0.0.1:5901）==="
    echo "编译内核..."
    CHAPTER=game cargo build 2>&1 | tail -1
    if [[ ! -f "$FS_IMG" ]]; then
      echo "缺少磁盘镜像: $FS_IMG（请先在本目录执行 cargo build）" >&2
      exit 1
    fi
    exec qemu-system-riscv64 \
      -machine virt -smp 1 -m 128M -bios none \
      -drive "file=$FS_IMG,if=none,format=raw,id=x0" \
      -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
      -device virtio-gpu-device \
      -vnc :1 \
      -serial stdio \
      -kernel "$KERNEL"
    ;;
  gui)
    echo "=== ch8: 本地图形窗口（GTK）==="
    if [[ -z "${DISPLAY:-}" ]]; then
      echo "未设置 DISPLAY。请登录图形桌面或: export DISPLAY=:0" >&2
      echo "远程可用: $0 vnc" >&2
      exit 1
    fi
    echo "编译内核..."
    CHAPTER=game cargo build 2>&1 | tail -1
    if [[ ! -f "$FS_IMG" ]]; then
      echo "缺少磁盘镜像: $FS_IMG" >&2
      exit 1
    fi
    exec qemu-system-riscv64 \
      -machine virt -smp 1 -m 128M -bios none \
      -drive "file=$FS_IMG,if=none,format=raw,id=x0" \
      -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
      -device virtio-gpu-device \
      -display gtk \
      -serial stdio \
      -kernel "$KERNEL"
    ;;
  demo)
    echo "=== ch8: 自动演示（CHAPTER=game → doom + monitor 截图）==="
    export CHAPTER=game
    echo "编译（用户 initproc → doom）..."
    CHAPTER=game cargo build 2>&1 | tail -1
    rm -f "$SCREENSHOT"
    echo "启动 QEMU (VirtIO-GPU, 无本地窗口)..."
    qemu-system-riscv64 \
      -machine virt -smp 1 -m 128M -bios none \
      -drive "file=$FS_IMG,if=none,format=raw,id=x0" \
      -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
      -device virtio-gpu-device \
      -display none \
      -serial stdio \
      -monitor "telnet:127.0.0.1:$MON_PORT,server,nowait" \
      -kernel "$KERNEL" &
    QEMU_PID=$!
    sleep 8
    for _ in $(seq 1 12); do
      if kill -0 "$QEMU_PID" 2>/dev/null; then
        (echo "sendkey w"; sleep 0.2
         echo "sendkey d"; sleep 0.2
         echo "screendump $SCREENSHOT"; sleep 0.5) | nc -q 1 127.0.0.1 "$MON_PORT" 2>/dev/null && break || true
        sleep 1
      else
        break
      fi
    done
    sleep 1
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    if [ -f "$SCREENSHOT" ]; then
      echo "=== 截图: $SCREENSHOT ==="
    else
      echo "警告: 截图未生成（需 nc 连 QEMU monitor）"
    fi
    ;;
  help|-h|--help)
    usage
    ;;
  default|*)
    echo "=== ch8-doom: DOOM 风格演示 (VirtIO-GPU) ==="
    usage
    echo "编译内核..."
    CHAPTER=game cargo build 2>&1 | tail -1

    rm -f "$SCREENSHOT"

    echo "启动 QEMU (VirtIO-GPU)..."
    qemu-system-riscv64 \
      -machine virt -smp 1 -m 128M -bios none \
      -drive "file=$FS_IMG,if=none,format=raw,id=x0" \
      -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
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
      echo "警告: 截图未生成"
    fi
    ;;
esac

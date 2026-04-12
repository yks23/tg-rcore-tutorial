#!/usr/bin/env bash
set -euo pipefail

# 使用绝对路径：QEMU 对 -drive file= 中含「可执行文件名/../fs.img」的解析会触发 ENOTDIR。
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FS_IMG="$ROOT/target/riscv64gc-unknown-none-elf/debug/fs.img"
KERNEL="$ROOT/target/riscv64gc-unknown-none-elf/debug/tg-rcore-tutorial-ch6-yks23"
SCREENSHOT="$ROOT/screenshot-breakout.ppm"
MON_PORT=44406

usage() {
  echo "用法: $0 [default|vnc|gui|demo]"
  echo "  default  无头 + monitor 截图"
  echo "  vnc      VNC :1 → 127.0.0.1:5901（SSH: ssh -L 5901:127.0.0.1:5901 user@host）"
  echo "  gui      本机有图形界面时弹出 QEMU 窗口（需 DISPLAY）"
  echo "  demo     CHAPTER=game → breakout + 自动截图"
}

mode="${1:-default}"

case "$mode" in
  vnc)
    echo "=== ch6: VNC（127.0.0.1:5901）==="
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
    echo "=== ch6: 本地图形窗口（GTK）==="
    if [[ -z "${DISPLAY:-}" ]]; then
      echo "未设置 DISPLAY。请登录图形桌面或: export DISPLAY=:0" >&2
      echo "远程可用: $0 vnc" >&2
      exit 1
    fi
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
    echo "=== ch6: 自动演示（CHAPTER=game → breakout + 截图）==="
    export CHAPTER=game
    cargo build 2>&1 | tail -1
    rm -f "$SCREENSHOT"
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
    (echo "sendkey a"; sleep 0.2; echo "screendump $SCREENSHOT"; sleep 0.5) | nc -q 1 127.0.0.1 "$MON_PORT" 2>/dev/null || true
    sleep 1
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    [[ -f "$SCREENSHOT" ]] && echo "截图: $SCREENSHOT" || echo "截图未生成"
    ;;
  help|-h|--help)
    usage
    ;;
  default|*)
    echo "=== ch6-breakout: 打砖块 (VirtIO-GPU) ==="
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

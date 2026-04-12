# 实验 9：ch1 多核引导（T2L9）

## 目标

在 ch1 最简裸机程序（`Hello, world!`）的基础上，实现 **SMP 多核引导**：
- 每个 hart 拥有独立的 S 态引导栈（`STACKS[hartid]`）
- 副核（hart 1..7）打印上线消息后进入 `wfi` 等待
- 主核（hart 0）等待所有副核上线后打印 `Hello, world!` 并关机
- 通过 `CONSOLE_LOCK` 自旋锁串行化 UART 输出，避免字节交错

## 额外实现（T3L1 叠加）

在 T2L9 基础上，本 crate 还实现了 **T3L1**：通过 VirtIO-GPU Framebuffer 绘制七巧板 "OS" 图案。
运行 `bash show.sh default` 可查看图形输出。

## 如何验收

在 **本目录** 执行：

```bash
bash test-smp.sh
```

无交互运行：`SMPTEST_QUICK=1 bash test-smp.sh`

**多核**（`RCORE_SMP=8`）应看到：
1. `[qemu-virt-kernel] RCORE_SMP=8 ...`
2. 连续 7 行：`[ch1] hart 1~7 secondary online`（顺序可乱）
3. 最后一行：`Hello, world!`

**单核**（`RCORE_SMP=1`）应看到：
1. `[qemu-virt-kernel] RCORE_SMP=1 ...`
2. 无副核上线行
3. `Hello, world!`

## 设计说明

| 要点 | 说明 |
|------|------|
| `write_qemu_smp` | `build.rs` 读取环境变量 `RCORE_SMP`，生成 `OUT_DIR/qemu_smp.rs`，内含 `pub const QEMU_SMP: usize = N`。 |
| `STACKS[hartid]` | `_start` 裸函数中，通过 `a0`（hartid）计算偏移 `hartid << PAGE_BITS`，设置独立栈顶。 |
| `CONSOLE_LOCK` | `AtomicBool` 实现的自旋锁，保护 `console_putchar` 的整段输出，防止多核 UART 字节交错。 |
| `SECONDARIES_PRINTED` | `AtomicUsize` 计数器，hart 0 等待所有副核计数到 `QEMU_SMP - 1` 后才继续。 |
| 副核 | 打印上线行后 `fetch_add(1)` 并进入 `loop { wfi }`。 |

完整报告见仓库 `report/T2L9.md`（如存在）。

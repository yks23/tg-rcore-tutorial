# 实验 10：ch3 多核引导（T2L10）

## 目标

在保持 **调度、时钟中断、用户任务仅在 hart 0** 的前提下，完成与 ch2 同型的 **SMP 引导**：每核栈、`BSS_READY`、`QEMU_SMP`（`build.rs` + `RCORE_SMP`）、副核上线行、`CONSOLE_LOCK`。

## 如何验收

在 **本目录** 执行：

```bash
./verify.sh
```

无交互：`SMPTEST_QUICK=1 ./verify.sh`。

**多核**：stderr 有 `RCORE_SMP=8`；开头 **7 行** `[ch3] hart … secondary (scheduler on hart 0 only)`；随后才是 `LOG TEST`、`load app…`。  
**单核**：`RCORE_SMP=1`，**无**上述 7 行。

## 设计说明（报告可摘）

| 要点 | 说明 |
|------|------|
| `write_qemu_smp` | `build.rs` 生成 `OUT_DIR/qemu_smp.rs` 中的 `QEMU_SMP`，与 `scripts/qemu-virt-kernel.sh` 的 `-smp` 一致。 |
| `BSS_READY` | hart 0 `zero_bss` 后置位；副核在此之前不访问 BSS 内原子变量。 |
| `BOOT_DONE` | 仍表示「hart 0 已完成控制台与用户程序加载、即将开时钟/进主循环」类语义；副核**不再**依赖它打印上线行。 |
| 副核 | 打印后 `SECONDARIES_ONLINE++`，然后 `wfi`。 |

完整报告与 Bug 表见仓库根 [`report/T2L10.md`](../report/T2L10.md)。

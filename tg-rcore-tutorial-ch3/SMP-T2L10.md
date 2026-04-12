# 实验 10：ch3 多核调度（T2L10，增强版）

## 目标（v0.4.8-preview.2 升级）

**基础目标（T2L10）**：SMP 多核引导，每核独立栈，副核上线后等待调度。

**增强实现**：副核不再仅停在 `wfi`，而是通过 **全局大锁（BKL）** 真正参与分时调度：
- 所有 hart 共同执行 `sched_loop()`
- `SCHED_LOCK`（`AtomicBool` 自旋锁）保护 TCB 选取，防止两核同时运行同一任务
- `ROUND_IDX`（`AtomicUsize`）原子轮转，实现多核 Round-Robin
- `BOOT_DONE` 后主核 `send_ipi(all_secondaries)` 唤醒副核进入调度

**实测现象**（smp=2）：
```
[ INFO] hart0 app9 exit(0)   ← hart0 在调度
[ INFO] hart1 app6 exit(0)   ← hart1 也在调度！
```

## 如何验收

在 **本目录** 执行：

```bash
RCORE_SMP=2 ./verify.sh   # 2核并行调度
RCORE_SMP=8 ./verify.sh   # 8核并行调度
RCORE_SMP=1 ./verify.sh   # 单核对照
```

## 设计说明

| 要点 | 说明 |
|------|------|
| `write_qemu_smp` | `build.rs` 生成 `QEMU_SMP` 常量（读 `RCORE_SMP` 环境变量）。 |
| `BSS_READY` | hart 0 `zero_bss` 后置位；副核等此信号再访问原子变量。 |
| `BOOT_DONE` + `send_ipi` | 主核初始化完成后广播 IPI 唤醒所有副核（写 CLINT msip 寄存器）。 |
| `SCHED_LOCK` | `AtomicBool` 全局大锁，覆盖"取任务 → execute → 处理 Trap"完整周期。 |
| `ROUND_IDX` | `AtomicUsize` 原子递增，多核间公平轮转任务槽位。 |
| `REMAIN` | 原子计数器，归零时所有核退出调度循环并关机。 |


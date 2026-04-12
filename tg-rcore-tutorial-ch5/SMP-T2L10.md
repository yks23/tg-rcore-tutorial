# 实验 10：ch5 多核进程调度（T2L10，增强版）

## 目标（v0.4.8-preview.2 升级）

**基础目标（T2L10）**：SMP 多核引导，Sv39 页表、Stride 调度、initproc 正确建立，副核上线后待命。

**增强实现**：副核通过 **全局大锁（BKL）** 真正参与进程调度：
- `SCHED_LOCK`（`spin::Mutex<()>`）包裹 `PROCESSOR.find_next/make_current`
- `run_sched_loop(portal)` 被主核和副核统一调用
- 副核通过 `PROTAL_TRANSIT` 恒等映射地址重获 portal 引用（内核恒等映射，安全）
- 主核初始化完成后 `send_ipi(all_secondaries)` 唤醒副核

**实测现象**（smp=2）：`hart1` 打印 "joining scheduler"，用户 shell 正常运行。

## 如何验收

```bash
RCORE_SMP=2 ./verify.sh   # 2核并行进程调度
RCORE_SMP=8 ./verify.sh   # 8核
RCORE_SMP=1 ./verify.sh   # 单核对照
```

**多核**：开头 **N-1 行** `[ch5] hart … secondary online, joining scheduler`，随后进入 shell 逻辑。  
**单核**：无副核行，直接看到 shell。

## 设计说明

| 要点 | 说明 |
|------|------|
| `SCHED_LOCK` | `spin::Mutex<()>` 全局大锁，保证 `PROCESSOR`（UnsafeCell 包装）的互斥访问。 |
| `run_sched_loop` | 共享调度函数：lock → find_next → execute → trap → unlock，各核平等竞争。 |
| `sched_loop_secondary` | 副核入口：从 `PROTAL_TRANSIT` 地址重获 portal，调用 `run_sched_loop`。 |
| `send_ipi` | 主核 BOOT_DONE 后广播 IPI（CLINT msip）唤醒所有副核。 |

注意：ch5 交互与程序较多，单次 `cargo run` 可能很长。

## 实现要点

`BSS_READY` 必须在 **`zero_bss` 之后**再放开副核：ch5 含 `Lazy` 等 BSS 内结构，副核绝不能早于 hart 0 的 `zero_bss` 访问这些全局状态。

报告：[`report/T2L10.md`](../report/T2L10.md)。

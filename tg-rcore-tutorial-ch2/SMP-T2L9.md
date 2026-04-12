# 实验 9：ch2 多核引导（T2L9）

## 目标

在 ch2 批处理系统的基础上，完成 **SMP 多核引导**：
- 每个 hart 拥有独立的 S 态引导栈（`STACKS[hartid]`）
- 副核打印上线消息后进入 `wfi`（批处理应用仍仅在 hart 0 执行）
- 主核等待所有副核上线后再开始批处理
- `BSS_READY` / `SECONDARIES_ONLINE` / `CONSOLE_LOCK` 全套原子同步

## 如何验收

在 **本目录** 执行：

```bash
bash test-smp.sh
```

无交互运行：`SMPTEST_QUICK=1 bash test-smp.sh`

**多核**（`RCORE_SMP=8`）应看到：
1. `[qemu-virt-kernel] RCORE_SMP=8 ...`
2. 连续 7 行：`[ch2] hart … secondary (batch on hart 0 only)`
3. 随后才是 ASCII 横幅、`[TRACE] LOG TEST`、`load app…`

**单核**（`RCORE_SMP=1`）应看到：
1. `[qemu-virt-kernel] RCORE_SMP=1`
2. 无副核上线行
3. 直接看到横幅和 app 加载

## 设计说明

| 要点 | 说明 |
|------|------|
| `write_qemu_smp` | `build.rs` 根据 `RCORE_SMP` 生成 `QEMU_SMP` 常量，控制等待的副核数。 |
| `BSS_READY` | hart 0 `zero_bss()` 完成后置位；副核必须先等它，再访问 BSS 内原子变量。 |
| `SECONDARIES_ONLINE` | 计数器；hart 0 等到 `QEMU_SMP - 1` 后才初始化控制台。 |
| `CONSOLE_LOCK` | `AtomicBool` 自旋锁，串行化 `console_putchar` 防止 UART 字节交错。 |
| 副核 | `BSS_READY` 就绪 → 打印上线行 → `SECONDARIES_ONLINE++` → `wfi`。 |
| 批处理仍单核 | 调度逻辑未并行化，仅多核引导点火，符合 T2L9 实验目标。 |

# 实验 10：ch4 多核引导（T2L10）

## 目标

多核 **QEMU `-smp`** 下正确引导；**Sv39、异界传送门、调度线程仍在 hart 0**；副核仅打印上线信息后 `wfi`。

## 如何验收

```bash
./verify.sh
```

**多核**：`RCORE_SMP=8` 时开头 **7 行** `[ch4] hart … secondary …`，其后为横幅 / `LOG TEST` / `detect app`。  
**单核**：`RCORE_SMP=1` 时无 7 行副核提示。

ch4 全量运行时间较长，请耐心等待或按 QEMU 说明退出。

## 实现要点

与 ch3 相同模式：`BSS_READY`、`CONSOLE_LOCK`、`SECONDARIES_ONLINE`、`QEMU_SMP`；hart 0 在 `init_console` **之前**等到 `QEMU_SMP - 1` 个副核打完字。

报告：[`report/T2L10.md`](../report/T2L10.md)。

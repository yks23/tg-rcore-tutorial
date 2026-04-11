# 实验 10：ch5 多核引导（T2L10）

## 目标

多核下正确进入 **进程管理 / Stride / initproc**；**仅 hart 0** 执行主调度循环；副核 `wfi`。

## 如何验收

```bash
./verify.sh
```

**多核**：开头 **7 行** `[ch5] hart … secondary …`，再进入用户态 shell 等逻辑。  
**单核**：无 7 行。

注意：ch5 交互与程序较多，单次 `cargo run` 可能很长。

## 实现要点

`BSS_READY` 必须在 **`zero_bss` 之后**再放开副核：ch5 含 `Lazy` 等 BSS 内结构，副核绝不能早于 hart 0 的 `zero_bss` 访问这些全局状态。

报告：[`report/T2L10.md`](../report/T2L10.md)。

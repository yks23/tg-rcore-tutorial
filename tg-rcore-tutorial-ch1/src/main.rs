//! # 第一章：应用程序与基本执行环境
//!
//! 本章实现了一个最简单的 RISC-V S 态裸机程序，展示操作系统的最小执行环境。
//!
//! ## 关键概念
//!
//! - `#![no_std]`：不使用 Rust 标准库，改用不依赖操作系统的核心库 `core`
//! - `#![no_main]`：不使用标准的 `main` 入口，自定义裸函数 `_start` 作为入口
//! - 裸函数（naked function）：不生成函数序言/尾声，可在无栈环境下执行
//! - SBI（Supervisor Binary Interface）：S 态软件向 M 态固件请求服务的标准接口
//!
//! 教程阅读建议：
//!
//! - 先看 `_start`：理解无运行时情况下的最小启动流程；
//! - 再看 `rust_main`：理解最小 I/O 路径（SBI 输出 + 关机）；
//! - 最后看 `panic_handler`：理解 no_std 程序的异常收口方式。

// 不使用标准库，因为裸机环境没有操作系统提供系统调用支持
#![no_std]
// 不使用标准入口，因为裸机环境没有 C runtime 进行初始化
#![no_main]
// RISC-V64 架构下启用严格警告和文档检查
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
// 非 RISC-V64 架构允许死代码（用于 cargo publish --dry-run 在主机上通过编译）
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]

// 引入 SBI 调用库，提供 console_putchar（输出字符）和 shutdown（关机）功能
// 启用 nobios 特性后，tg_sbi 内建了 M-mode 启动代码，无需外部 SBI 固件
#[cfg(target_arch = "riscv64")]
include!(concat!(env!("OUT_DIR"), "/qemu_smp.rs"));

#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(target_arch = "riscv64")]
use tg_sbi::console_putchar;
use tg_sbi::shutdown;

/// 串口逐字输出无原子性，多核同时写会交错；实验 9 验收时用自旋锁串行化整段输出。
#[cfg(target_arch = "riscv64")]
static CONSOLE_LOCK: AtomicBool = AtomicBool::new(false);

/// 已完成「副核上线」一行的 hart 数量（hart 0 等待到 `QEMU_SMP - 1` 再打印并关机；`QEMU_SMP=1` 时不等待）。
#[cfg(target_arch = "riscv64")]
static SECONDARIES_PRINTED: AtomicUsize = AtomicUsize::new(0);

/// 获取串口锁后执行 `f`（整段消息期间独占 `console_putchar`）。
#[cfg(target_arch = "riscv64")]
fn with_console_lock(mut f: impl FnMut()) {
    while CONSOLE_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    f();
    CONSOLE_LOCK.store(false, Ordering::Release);
}

/// S 态程序入口点。
///
/// 这是一个裸函数（naked function），放置在 `.text.entry` 段，
/// 链接脚本将其安排在地址 `0x80200000`。
///
/// 裸函数不生成函数序言和尾声，因此可以在没有栈的情况下执行。
/// 它完成两件事：
/// 1. 设置栈指针 `sp`，指向栈顶（栈从高地址向低地址增长）
/// 2. 跳转到 Rust 主函数 `rust_main`
/// 多核启动时 S 态每核引导栈数量（与 `m_entry.asm` 中 M 栈 hart 上限一致）。
#[cfg(target_arch = "riscv64")]
const SMP_HART_MAX: usize = 8;

#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = 4096;

    #[unsafe(link_section = ".bss.uninit")]
    static mut STACKS: [[u8; STACK_SIZE]; SMP_HART_MAX] = [[0u8; STACK_SIZE]; SMP_HART_MAX];

    core::arch::naked_asm!(
        // a0 = mhartid（由 tg-sbi m_entry 在 mret 前传入）
        "li     t1, {max_harts}",
        "bgeu   a0, t1, 9f",
        "la     t2, {stacks}",
        "slli   t3, a0, 12",
        "add    t2, t2, t3",
        "li     t1, {stack_size}",
        "add    sp, t2, t1",
        "j      {main}",
        "9:",
        "1:",
        "j      1b",
        max_harts = const SMP_HART_MAX,
        stack_size = const STACK_SIZE,
        stacks = sym STACKS,
        main = sym rust_main,
    )
}

/// 无堆十进制输出（供副核打印 `hartid`）。
#[cfg(target_arch = "riscv64")]
fn put_decimal(mut n: usize) {
    if n == 0 {
        console_putchar(b'0');
        return;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        i += 1;
        n /= 10;
    }
    while i > 0 {
        i -= 1;
        console_putchar(tmp[i]);
    }
}

/// S 态主函数：打印 "Hello, world!" 并关机；副核仅打印上线信息后 `wfi` 等待（实验 9：ch1 多核）。
///
/// `hartid` 由 M 态 `mret` 经 `a0` 传入（见 `tg-sbi` `m_entry.asm`）。
#[cfg(target_arch = "riscv64")]
extern "C" fn rust_main(hartid: usize) -> ! {
    if hartid != 0 {
        with_console_lock(|| {
            for c in b"[ch1] hart " {
                console_putchar(*c);
            }
            put_decimal(hartid);
            for c in b" secondary online\n" {
                console_putchar(*c);
            }
        });
        SECONDARIES_PRINTED.fetch_add(1, Ordering::Release);
        loop {
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
        }
    }
    let need_secondaries = QEMU_SMP.saturating_sub(1);
    while SECONDARIES_PRINTED.load(Ordering::Acquire) < need_secondaries {
        core::hint::spin_loop();
    }
    with_console_lock(|| {
        for c in b"Hello, world!\n" {
            console_putchar(*c);
        }
    });
    shutdown(false) // false 表示正常关机
}

/// panic 处理函数。
///
/// `#![no_std]` 环境下必须自行实现。发生 panic 时以异常状态关机。
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    shutdown(true) // true 表示异常关机
}

/// 非 RISC-V64 架构的占位模块。
///
/// 提供 `main` 等符号，使得在主机平台（如 x86_64）上也能通过编译，
/// 满足 `cargo publish --dry-run` 和 `cargo test` 的需求。
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    /// 主机平台占位入口
    #[unsafe(no_mangle)]
    pub extern "C" fn main() -> i32 {
        0
    }

    /// C 运行时占位
    #[unsafe(no_mangle)]
    pub extern "C" fn __libc_start_main() -> i32 {
        0
    }

    /// Rust 异常处理人格占位
    #[unsafe(no_mangle)]
    pub extern "C" fn rust_eh_personality() {}
}

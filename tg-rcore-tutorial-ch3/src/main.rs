//! # 第三章：多道程序与分时多任务
//!
//! 本章在第二章"批处理系统"的基础上，实现了一个**多道程序操作系统**，
//! 支持多个用户程序并发执行，并通过时钟中断实现抢占式调度。
//!
//! ## 核心概念
//!
//! - **多道程序**：多个用户程序同时驻留在内存中，内核在它们之间切换执行
//! - **任务控制块（TCB）**：管理每个任务的上下文、状态和资源
//! - **协作式调度**：任务通过 `yield` 系统调用主动让出 CPU
//! - **抢占式调度**：通过时钟中断强制切换任务，实现时间片轮转
//! - **系统调用**：`write`、`exit`、`yield`、`clock_gettime`
//!
//! 教程阅读建议：
//!
//! - 先看 `rust_main` 主循环：理解“轮转 + 时钟中断 + ecall”三类事件交织；
//! - 再看 `TaskControlBlock` 的使用方式：理解任务上下文与生命周期；
//! - 最后看 `impls::Clock`：理解硬件时钟到用户态时间结构体的桥接。

// 不使用标准库，裸机环境没有操作系统提供系统调用支持
#![no_std]
// 不使用标准入口，裸机环境没有 C runtime 进行初始化
#![no_main]
// RISC-V64 架构下启用严格警告和文档检查
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
// 非 RISC-V64 架构允许死代码（用于 cargo publish --dry-run 在主机上通过编译）
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]

// 任务管理模块：定义任务控制块（TCB）和调度事件
mod task;

// 引入控制台输出宏（print! / println!），由 tg_console 库提供
#[macro_use]
extern crate tg_console;

// 本地模块：Console 和 SyscallContext 的实现
use impls::{Console, SyscallContext};
// riscv 库：访问 RISC-V 控制状态寄存器（CSR），如 scause、sie、time
use riscv::register::*;
// 任务控制块
use task::TaskControlBlock;
// 日志模块
use tg_console::log;
// SBI 调用：set_timer、console_putchar、shutdown 等
use tg_sbi;
#[cfg(target_arch = "riscv64")]
include!(concat!(env!("OUT_DIR"), "/qemu_smp.rs"));
#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ========== 启动相关 ==========

// 将用户程序的二进制数据内联到内核镜像的 .data 段中
// APP_ASM 由 build.rs 在编译时生成，包含所有用户程序的二进制数据
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!(env!("APP_ASM")));

// 最大支持的应用程序数量
const APP_CAPACITY: usize = 32;

/// 任务控制块表放在静态存储区，避免在 `rust_main` 栈上分配
/// `32 × (用户栈 8KiB + syscall 计数表等)` 导致栈溢出。
static mut KERNEL_TCBS: [TaskControlBlock; APP_CAPACITY] = [TaskControlBlock::ZERO; APP_CAPACITY];

/// hart 0 完成 `zero_bss` 后置位；副核在此之前不得访问本文件 BSS 内同步变量（实验 10）。
#[cfg(target_arch = "riscv64")]
static BSS_READY: AtomicBool = AtomicBool::new(false);

/// 串行化 `console_putchar`，避免多核 UART 字节交错。
#[cfg(target_arch = "riscv64")]
static CONSOLE_LOCK: AtomicBool = AtomicBool::new(false);

/// 已打印「副核上线」的从核个数；hart 0 在 `init_console` 前等到 `QEMU_SMP - 1`（单核为 0）。
#[cfg(target_arch = "riscv64")]
static SECONDARIES_ONLINE: AtomicUsize = AtomicUsize::new(0);

/// hart 0 完成全局初始化与时钟中断开启后置位；供需要「全系统就绪」语义时使用（实验 10）。
#[cfg(target_arch = "riscv64")]
static BOOT_DONE: AtomicBool = AtomicBool::new(false);

/// 多核引导栈槽数量（与 `tg-sbi` M 态一致）。
#[cfg(target_arch = "riscv64")]
const SMP_HART_MAX: usize = 8;

// 定义内核入口点：分配 (APP_CAPACITY + 2) * 8 KiB = 272 KiB 的内核栈
// 比第二章更大，因为需要同时容纳多个任务的内核上下文。
//
// 这里不再调用 tg_linker::boot0! 宏，避免外部已发布版本与 Rust 2024
// 在属性语义上的兼容差异影响本 crate 的发布校验。
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = (APP_CAPACITY + 2) * 8192;
    #[unsafe(link_section = ".boot.stack")]
    static mut STACKS: [[u8; STACK_SIZE]; SMP_HART_MAX] =
        [[0u8; STACK_SIZE]; SMP_HART_MAX];

    core::arch::naked_asm!(
        "li     t1, {max_harts}",
        "bgeu   a0, t1, 9f",
        "la     t2, {stacks}",
        "slli   t3, a0, 13",
        "slli   t4, t3, 5",
        "slli   t5, t3, 1",
        "add    t3, t4, t5",
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

#[cfg(target_arch = "riscv64")]
/// 通过直接写 CLINT msip 寄存器向目标 hart 发送核间中断（IPI），唤醒 wfi 中的副核。
/// QEMU virt 平台 CLINT msip 基地址 0x200_0000，每核 4 字节，写 1 触发 M 态软件中断。
#[cfg(target_arch = "riscv64")]
fn send_ipi_clint(hart_mask: usize) {
    const CLINT_MSIP_BASE: usize = 0x200_0000;
    for hart_id in 0..8usize {
        if hart_mask & (1 << hart_id) != 0 {
            unsafe { ((CLINT_MSIP_BASE + hart_id * 4) as *mut u32).write_volatile(1) };
        }
    }
}

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

/// 无堆十进制输出（副核在 `init_console` 前打印 hartid）。
#[cfg(target_arch = "riscv64")]
fn put_decimal_hart(mut n: usize) {
    use tg_sbi::console_putchar;
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

// ========== 内核主函数 ==========

/// 全局任务调度自旋锁：保护 `ROUND_IDX` 的取/置，确保多核间不会同时调度同一任务。
///
/// 设计原理：
/// - ch3 任务是"一次性"的 —— execute() 后立即处理 Trap，而非可抢占的进程。
/// - 因此自旋锁粒度可以覆盖整个"取任务 → 执行 → 处理 Trap"周期。
/// - 每次时钟中断或 yield 都会释放锁（因为 execute() 返回到锁释放处），
///   另一个 hart 立即可以抢入执行下一个就绪任务。
#[cfg(target_arch = "riscv64")]
static SCHED_LOCK: AtomicBool = AtomicBool::new(false);

/// 全局轮转索引（下一个应检查的 TCB 槽位）。
#[cfg(target_arch = "riscv64")]
static ROUND_IDX: AtomicUsize = AtomicUsize::new(0);

/// 全局任务总数（主核初始化后写入，副核只读）。
#[cfg(target_arch = "riscv64")]
static TASK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 全局未完成任务计数（归零时所有核退出调度循环）。
#[cfg(target_arch = "riscv64")]
static REMAIN: AtomicUsize = AtomicUsize::new(0);

/// 共享调度循环：主核/副核在初始化完毕后均调用此函数，实现多核并行分时调度。
///
/// ### 调度策略（全局大锁 BKL）
///
/// 最简多核方案——全局自旋锁（类似 Linux 早期的 Big Kernel Lock）：
/// 1. 加 SCHED_LOCK
/// 2. 轮转找一个未完成任务（原子递增 ROUND_IDX，取模 n）
/// 3. 若找到：执行任务的一个完整"时间片"（一次 execute 直到 Trap 返回）
/// 4. 处理 Trap，必要时标记任务完成（递减 REMAIN）
/// 5. 释放锁 → 下一轮从步骤 1 开始（其他核可立即抢入）
///
/// ### 关键特性
/// - 任务粒度串行：每个时间片只有一个核在执行用户任务，不会出现同一任务
///   被两个核同时运行的情况
/// - 无任务时 wfi 睡眠，避免忙等耗尽总线带宽
/// - REMAIN == 0 时所有核退出并关机
#[cfg(target_arch = "riscv64")]
fn sched_loop() -> ! {
    let n = TASK_COUNT.load(Ordering::Acquire);
    loop {
        if REMAIN.load(Ordering::Relaxed) == 0 {
            break;
        }

        // ── 加调度锁：确保任务选取和执行的原子性 ──
        while SCHED_LOCK
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            if REMAIN.load(Ordering::Relaxed) == 0 {
                tg_sbi::shutdown(false);
            }
            core::hint::spin_loop();
        }

        // ── 轮转找下一个未完成任务 ──
        // 原子递增保证多核间不会选中同一个槽位（即使出现，finish 标志也会过滤）
        let tcbs = unsafe { &mut KERNEL_TCBS[..n] };
        let mut found_idx: Option<usize> = None;
        for _ in 0..n {
            let i = ROUND_IDX.fetch_add(1, Ordering::Relaxed) % n;
            if !tcbs[i].finish {
                found_idx = Some(i);
                break;
            }
        }

        if let Some(i) = found_idx {
            let tcb = &mut tcbs[i];

            // ── 执行一个时间片（持锁期间，单核独占此任务）──
            loop {
                #[cfg(not(feature = "coop"))]
                tg_sbi::set_timer(riscv::register::time::read64() + 12500);

                unsafe { tcb.execute() };

                use scause::*;
                let (done, should_switch) = match scause::read().cause() {
                    Trap::Interrupt(Interrupt::SupervisorTimer) => {
                        tg_sbi::set_timer(u64::MAX);
                        log::trace!("hart{} app{i} timeout", hartid_of_current());
                        (false, true) // 时间片到期，切换到下一任务
                    }
                    Trap::Exception(Exception::UserEnvCall) => {
                        use task::SchedulingEvent as Event;
                        match tcb.handle_syscall(i) {
                            Event::None => (false, false), // 继续执行当前任务
                            Event::Exit(code) => {
                                log::info!("hart{} app{i} exit({code})", hartid_of_current());
                                (true, true)
                            }
                            Event::Yield => {
                                log::debug!("hart{} app{i} yield", hartid_of_current());
                                (false, true) // 主动让出
                            }
                            Event::UnsupportedSyscall(id) => {
                                log::error!("hart{} app{i} unsupported syscall {}", hartid_of_current(), id.0);
                                (true, true)
                            }
                        }
                    }
                    Trap::Exception(e) => {
                        log::error!("hart{} app{i} killed by {e:?}", hartid_of_current());
                        (true, true)
                    }
                    Trap::Interrupt(ir) => {
                        log::error!("hart{} app{i} killed by {ir:?}", hartid_of_current());
                        (true, true)
                    }
                };

                if done {
                    tcb.finish = true;
                    REMAIN.fetch_sub(1, Ordering::Release);
                }
                if should_switch { break; }
            }
        } else {
            // 无可运行任务（可能全部完成或暂时阻塞）
            SCHED_LOCK.store(false, Ordering::Release);
            if REMAIN.load(Ordering::Relaxed) == 0 { break; }
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
            continue;
        }

        // ── 释放锁，允许其他核抢入 ──
        SCHED_LOCK.store(false, Ordering::Release);
    }

    tg_sbi::shutdown(false)
}

/// 读取当前 hart id（通过 CSR mhartid 的 S 态镜像——在 nobios 环境下可通过 tp 或从
/// tg-sbi 传入值推断，此处直接使用 Rust 闭包捕获的 hartid 形参）。
/// 这里提供一个占位实现，实际 hartid 在 sched_loop 的调用位置已知。
#[cfg(target_arch = "riscv64")]
#[inline(always)]
fn hartid_of_current() -> usize {
    // 在 RISC-V S 态读取 mhartid 会触发非法指令异常（M 态寄存器）。
    // 实际 hartid 已由 tg-sbi 通过 a0 传入 rust_main，使用 thread-local 或
    // 从调用栈获取。这里用 0 占位，日志不要求精确 hart id 时可接受。
    // 精确实现：用 TP 寄存器存储 hartid（见 ch8 扩展）。
    riscv::register::sscratch::read() // 临时用 sscratch 存 hartid（见 rust_main 中设置）
}

/// 内核主函数：初始化各子系统，然后以多道方式并发运行所有用户程序。
///
/// 与第二章的串行批处理不同，本章的多道程序系统支持：
/// - 多个任务同时驻留在内存中，每个任务拥有独立的 TCB 和用户栈
/// - 任务之间通过时间片轮转切换（抢占式调度，默认模式）
/// - 任务可以主动让出 CPU（协作式调度，通过 yield，需启用 `coop` feature）
///
/// **多核支持（SMP）**：
/// - 主核（hart 0）：完成全部初始化后，通过 IPI 唤醒所有副核，加入共享调度循环
/// - 副核（hart 1..N）：等待 BOOT_DONE，然后进入相同的 `sched_loop()` 并行执行任务
/// - 所有核共享全局 TCB 表，通过 `SCHED_LOCK` 自旋锁避免同时调度同一任务
#[cfg(target_arch = "riscv64")]
extern "C" fn rust_main(hartid: usize) -> ! {
    // 把 hartid 存入 sscratch，供 hartid_of_current() 读取（日志用）
    riscv::register::sscratch::write(hartid);

    if hartid != 0 {
        while !BSS_READY.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        use tg_sbi::console_putchar;
        with_console_lock(|| {
            for c in b"[ch3] hart " {
                console_putchar(*c);
            }
            put_decimal_hart(hartid);
            for c in b" secondary online, joining scheduler\n" {
                console_putchar(*c);
            }
        });
        SECONDARIES_ONLINE.fetch_add(1, Ordering::Release);

        // 副核等待主核完成全局初始化（BOOT_DONE），然后直接加入调度循环
        while !BOOT_DONE.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }

        // 副核开启 S 态时钟中断，支持抢占式调度
        #[cfg(not(feature = "coop"))]
        unsafe { riscv::register::sie::set_stimer() };

        // 进入共享调度循环（与主核平等地竞争任务）
        sched_loop()
    }

    // 第一步：清零 BSS 段（未初始化的全局变量区域）
    unsafe { tg_linker::KernelLayout::locate().zero_bss() };
    BSS_READY.store(true, Ordering::Release);

    let need_secondaries = QEMU_SMP.saturating_sub(1);
    while SECONDARIES_ONLINE.load(Ordering::Acquire) < need_secondaries {
        core::hint::spin_loop();
    }

    // 第二步：初始化控制台输出（使 print!/println! 可用）
    // 默认日志级别为 info（可通过 LOG 环境变量覆盖）
    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG").or(Some("info")));
    tg_console::test_log();

    // 第三步：初始化系统调用处理
    // 比第二章多了 scheduling（yield）、clock（获取时间）和 trace（追踪，练习题）
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    tg_syscall::init_scheduling(&SyscallContext);
    tg_syscall::init_clock(&SyscallContext);
    tg_syscall::init_trace(&SyscallContext);

    // 第四步：初始化任务控制块数组，加载所有用户程序
    let tcbs = unsafe { &mut KERNEL_TCBS[..] };
    let mut index_mod = 0;
    for (i, app) in tg_linker::AppMeta::locate().iter().enumerate() {
        let entry = app.as_ptr() as usize;
        log::info!("load app{i} to {entry:#x}");
        tcbs[i].init(entry);
        index_mod += 1;
    }
    println!();

    // 初始化全局调度计数器
    TASK_COUNT.store(index_mod, Ordering::Release);
    REMAIN.store(index_mod, Ordering::Release);

    // 发布 BOOT_DONE：副核看到此标志后立即加入调度
    BOOT_DONE.store(true, Ordering::Release);

    // 第五步：开启 S 特权级时钟中断
    #[cfg(not(feature = "coop"))]
    unsafe { sie::set_stimer() };

    // 第六步：通过 IPI 唤醒所有副核加入调度循环
    // hart_mask = 所有副核的位图（bit 1..QEMU_SMP-1 置位）
    let secondary_mask: usize = ((1usize << QEMU_SMP) - 1) & !1;
    if secondary_mask != 0 {
        send_ipi_clint(secondary_mask);
    }

    // 主核直接进入共享调度循环
    sched_loop()
}

// ========== panic 处理 ==========

/// panic 处理函数：打印错误信息后以异常状态关机。
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    tg_sbi::shutdown(true)
}

// ========== 接口实现 ==========

/// 各依赖库所需接口的具体实现
mod impls {
    use tg_syscall::*;

    /// 控制台实现：通过 SBI 逐字符输出
    pub struct Console;

    impl tg_console::Console for Console {
        #[inline]
        fn put_char(&self, c: u8) {
            tg_sbi::console_putchar(c);
        }
    }

    /// 系统调用上下文实现
    pub struct SyscallContext;

    /// IO 系统调用实现：处理 write 系统调用
    impl IO for SyscallContext {
        #[inline]
        fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            match fd {
                // 标准输出和调试输出：将缓冲区内容打印到控制台
                STDOUT | STDDEBUG => {
                    print!("{}", unsafe {
                        core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                            buf as *const u8,
                            count,
                        ))
                    });
                    count as _
                }
                _ => {
                    tg_console::log::error!("unsupported fd: {fd}");
                    -1
                }
            }
        }
    }

    /// Process 系统调用实现：处理 exit 系统调用
    impl Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: Caller, _status: usize) -> isize {
            0
        }
    }

    /// Scheduling 系统调用实现：处理 yield 系统调用
    ///
    /// `sched_yield` 允许任务主动让出 CPU，是协作式调度的基础。
    /// 内核在收到 yield 后会切换到下一个就绪任务。
    impl Scheduling for SyscallContext {
        #[inline]
        fn sched_yield(&self, _caller: Caller) -> isize {
            0
        }
    }

    /// Clock 系统调用实现：处理 clock_gettime 系统调用
    ///
    /// 将 RISC-V 硬件计时器的值转换为纳秒精度的时间。
    /// QEMU virt 平台的时钟频率为 12.5 MHz（10000/125 = 80 ns/tick）。
    impl Clock for SyscallContext {
        #[inline]
        fn clock_gettime(
            &self,
            _caller: Caller,
            clock_id: ClockId,
            tp: usize,
        ) -> isize {
            match clock_id {
                ClockId::CLOCK_MONOTONIC => {
                    // 将 RISC-V time 寄存器的值转换为纳秒
                    let time = riscv::register::time::read() * 10000 / 125;
                    *unsafe { &mut *(tp as *mut TimeSpec) } = TimeSpec {
                        tv_sec: time / 1_000_000_000,
                        tv_nsec: time % 1_000_000_000,
                    };
                    0
                }
                _ => -1,
            }
        }
    }

    /// Trace 系统调用实现（练习题需要完成的部分）
    ///
    /// 当前为占位实现，返回 -1 表示未实现。
    /// 学生需要在练习中实现 trace 功能，支持：
    /// - 读取用户内存（trace_request=0）
    /// - 写入用户内存（trace_request=1）
    /// - 查询系统调用计数（trace_request=2）
    impl Trace for SyscallContext {
        #[inline]
        fn trace(
            &self,
            _caller: Caller,
            trace_request: usize,
            id: usize,
            data: usize,
        ) -> isize {
            match trace_request {
                0 => {
                    // 读取当前任务用户地址 id 处一字节
                    unsafe { *{ id as *const u8 } as isize }
                }
                1 => {
                    // 写入 (data 低 8 位) 到用户地址 id
                    unsafe {
                        *(id as *mut u8) = data as u8;
                    }
                    0
                }
                2 => {
                    if id >= crate::task::SYSCALL_COUNT_LEN {
                        return -1;
                    }
                    crate::task::with_active_syscall_counts(|counts| counts[id] as isize)
                        .unwrap_or(-1)
                }
                _ => -1,
            }
        }
    }
}

/// 非 RISC-V64 架构的占位模块。
///
/// 提供编译所需的符号，使得 `cargo publish --dry-run` 在主机平台上能通过编译。
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

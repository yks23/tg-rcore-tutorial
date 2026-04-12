//! # 第五章：进程
//!
//! 本章在第四章"地址空间"的基础上，引入了完整的 **进程管理** 机制。
//! 进程是操作系统中最核心的抽象之一，它将"运行中的程序"封装为一个可管理的实体。
//!
//! ## 核心概念
//!
//! - **进程控制块（PCB）**：管理进程的所有资源（PID、地址空间、上下文、堆空间）
//! - **进程创建**：`fork` 系统调用复制父进程的地址空间，创建子进程
//! - **程序替换**：`exec` 系统调用加载新的 ELF 程序替换当前进程的地址空间
//! - **进程等待**：`wait` 系统调用等待子进程退出并回收资源
//! - **进程退出**：`exit` 系统调用终止当前进程
//! - **进程标识**：每个进程有唯一的 PID
//! - **进程树**：通过父子关系形成树状结构
//! - **初始进程（initproc）**：内核创建的第一个用户进程，是所有用户进程的祖先
//!
//! ## 与第四章的区别
//!
//! | 特性 | 第四章 | 第五章 |
//! |------|--------|--------|
//! | 进程创建 | 内核直接从 ELF 加载 | fork/exec 组合创建 |
//! | 进程关系 | 无父子关系 | 完整的进程树 |
//! | 资源回收 | 内核直接回收 | wait 系统调用回收 |
//! | 用户交互 | 无 | Shell 命令行界面 |
//! | 进程管理 | 简单列表 | ProcManager + 调度器 |
//!
//! 教程阅读建议：
//!
//! - 先看 `rust_main`：建立“initproc -> 调度 -> trap -> 状态迁移”的全局主线；
//! - 再看 `map_portal` 与 `kernel_space`：理解进程切换时地址空间一致性保障；
//! - 最后看 `impls::Process`：重点理解 fork/exec/wait 的语义边界。

// 不使用标准库，裸机环境没有操作系统提供系统调用支持
#![no_std]
// 不使用默认的 main 函数入口，裸机环境需要自定义入口点
#![no_main]
// 在 RISC-V 架构上启用严格的编译警告和文档要求
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
// 在非 RISC-V 架构上允许未使用的代码（用于 IDE 开发体验）
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code, unused_imports))]

/// 进程模块：定义 Process 结构体及其方法（from_elf、fork、exec 等）
mod process;
/// 处理器模块：定义 PROCESSOR 全局变量和进程管理器 ProcManager
mod processor;

#[macro_use]
extern crate tg_console;

extern crate alloc;

use crate::{
    impls::{Console, Sv39Manager, SyscallContext},
    process::Process,
    processor::{ProcManager, PROCESSOR},
};
use alloc::{alloc::alloc, collections::BTreeMap};
use core::{alloc::Layout, cell::UnsafeCell, ffi::CStr, mem::MaybeUninit};
use riscv::register::*;
use spin::Lazy;
#[cfg(not(target_arch = "riscv64"))]
use stub::Sv39;
use tg_console::log;
use tg_kernel_context::foreign::MultislotPortal;
#[cfg(target_arch = "riscv64")]
use tg_kernel_vm::page_table::Sv39;
use tg_kernel_vm::{
    page_table::{MmuMeta, VAddr, VmFlags, VmMeta, PPN, VPN},
    AddressSpace,
};
use tg_sbi;
use tg_syscall::Caller;
use tg_task_manage::{PManager, ProcId};
use xmas_elf::ElfFile;
#[cfg(target_arch = "riscv64")]
include!(concat!(env!("OUT_DIR"), "/qemu_smp.rs"));
#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 构建 VmFlags（虚拟内存标志位）。
///
/// 在 RISC-V 架构上使用编译期构建，参数格式如 `"U_WRV"` 表示：
/// - U: 用户态可访问
/// - W: 可写
/// - R: 可读
/// - V: 有效位
#[cfg(target_arch = "riscv64")]
const fn build_flags(s: &str) -> VmFlags<Sv39> {
    VmFlags::build_from_str(s)
}

/// 运行时解析 VmFlags 字符串。
#[cfg(target_arch = "riscv64")]
fn parse_flags(s: &str) -> Result<VmFlags<Sv39>, ()> {
    s.parse()
}

#[cfg(not(target_arch = "riscv64"))]
use stub::{build_flags, parse_flags};

// ─── 内联用户应用程序 ───
// build.rs 会编译用户程序并生成 APP_ASM 汇编文件，
// 通过 global_asm! 将所有用户程序的二进制数据嵌入内核镜像
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!(env!("APP_ASM")));

/// hart 0 完成 `zero_bss` 后置位；副核在此之前不得访问 BSS 内同步变量（实验 10）。
#[cfg(target_arch = "riscv64")]
static BSS_READY: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "riscv64")]
static CONSOLE_LOCK: AtomicBool = AtomicBool::new(false);

/// 调度自旋锁：保护全局 PROCESSOR 的 find_next/make_current 操作，实现多核安全调度。
///
/// ### 设计说明
///
/// ch5 的 PROCESSOR 内部使用 UnsafeCell 假设单核独占访问。
/// 引入此锁后，每个 hart 在调用 find_next/execute/handle_trap 全程持锁，
/// 从而将 PROCESSOR 的访问窗口串行化，无需改动 PROCESSOR 自身。
///
/// ### 性能特性
///
/// 锁粒度 = 一个完整的任务调度周期（find_next → execute → trap handling）。
/// 这意味着同一时刻只有一个 hart 在运行用户任务，其余 hart 在自旋等待。
/// 这与 Linux 早期的 BKL（Big Kernel Lock）原理相同，是多核调度的最简可行实现。
/// 后续章节可将粒度细化至 per-process 锁，实现真正并行。
#[cfg(target_arch = "riscv64")]
static SCHED_LOCK: spin::Mutex<()> = spin::Mutex::new(());

#[cfg(target_arch = "riscv64")]
static SECONDARIES_ONLINE: AtomicUsize = AtomicUsize::new(0);

/// hart 0 完成用户态子系统初始化、即将进入主调度循环前置位（实验 10）。
#[cfg(target_arch = "riscv64")]
static BOOT_DONE: AtomicBool = AtomicBool::new(false);

/// 多核引导栈槽数量（与 `tg-sbi` M 态一致）。
#[cfg(target_arch = "riscv64")]
const SMP_HART_MAX: usize = 8;

// 定义内核入口点，设置启动栈大小为 32 页 = 128 KiB。
//
// 这里不再调用 tg_linker::boot0! 宏，避免外部已发布版本与 Rust 2024
// 在属性语义上的兼容差异影响本 crate 的发布校验。
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = 32 * 4096;
    #[unsafe(link_section = ".boot.stack")]
    static mut STACKS: [[u8; STACK_SIZE]; SMP_HART_MAX] =
        [[0u8; STACK_SIZE]; SMP_HART_MAX];

    core::arch::naked_asm!(
        "li     t1, {max_harts}",
        "bgeu   a0, t1, 9f",
        "la     t2, {stacks}",
        "slli   t3, a0, 17",
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

/// 物理内存容量 = 48 MiB
const MEMORY: usize = 48 << 20;

/// 异界传送门所在虚页（虚拟地址空间最高页）
///
/// 传送门是一段特殊的代码页面，同时映射到内核和所有用户地址空间的相同虚拟地址，
/// 用于解决切换 satp（页表基地址）时代码地址失效的问题。
const PROTAL_TRANSIT: VPN<Sv39> = VPN::MAX;

/// 内核地址空间的全局存储
///
/// 使用 UnsafeCell + MaybeUninit 实现延迟初始化，
/// 因为内核地址空间需要在堆分配器初始化之后才能创建。
struct KernelSpace {
    inner: UnsafeCell<MaybeUninit<AddressSpace<Sv39, Sv39Manager>>>,
}

unsafe impl Sync for KernelSpace {}

impl KernelSpace {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// 写入内核地址空间（仅在初始化时调用一次）
    unsafe fn write(&self, space: AddressSpace<Sv39, Sv39Manager>) {
        unsafe { *self.inner.get() = MaybeUninit::new(space) };
    }

    /// 获取内核地址空间的不可变引用
    unsafe fn assume_init_ref(&self) -> &AddressSpace<Sv39, Sv39Manager> {
        unsafe { &*(*self.inner.get()).as_ptr() }
    }
}

/// 内核地址空间全局实例
static KERNEL_SPACE: KernelSpace = KernelSpace::new();

/// 应用程序名称到 ELF 数据的映射表
///
/// 在首次访问时通过 `Lazy` 初始化：
/// 1. 读取 build.rs 嵌入的应用元数据（`AppMeta`）获取各应用的二进制数据
/// 2. 读取 `app_names` 符号获取各应用的名称字符串
/// 3. 构建 BTreeMap 以支持按名称查找应用（供 exec 系统调用使用）
static APPS: Lazy<BTreeMap<&'static str, &'static [u8]>> = Lazy::new(|| {
    unsafe extern "C" {
        static app_names: u8;
    }
    unsafe {
        tg_linker::AppMeta::locate()
            .iter()
            .scan(&app_names as *const _ as usize, |addr, data| {
                let name = CStr::from_ptr(*addr as _).to_str().unwrap();
                *addr += name.as_bytes().len() + 1;
                Some((name, data))
            })
    }
    .collect()
});

/// 内核主函数——系统初始化和启动入口
///
/// 执行流程：
/// 1. 清零 BSS 段
/// 2. 初始化控制台和日志系统
/// 3. 初始化内核堆分配器
/// 4. 分配并创建异界传送门
/// 5. 建立内核地址空间（恒等映射），激活 Sv39 分页
/// 6. 初始化异界传送门和系统调用处理器
/// 7. 加载初始进程 `initproc`，进入调度循环
///
/// `hartid` 由 `tg-sbi` 经 `a0` 传入；仅 hart 0 跑主调度循环；副核经 `BSS_READY` 后打印上线行再 `wfi`（实验 10）。
#[cfg(target_arch = "riscv64")]
extern "C" fn rust_main(hartid: usize) -> ! {
    if hartid != 0 {
        while !BSS_READY.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        use tg_sbi::console_putchar;
        with_console_lock(|| {
            for c in b"[ch5] hart " {
                console_putchar(*c);
            }
            put_decimal_hart(hartid);
            for c in b" secondary online, joining scheduler\n" {
                console_putchar(*c);
            }
        });
        SECONDARIES_ONLINE.fetch_add(1, Ordering::Release);

        // 副核等待主核完成所有初始化（堆、地址空间、传送门均就绪）
        while !BOOT_DONE.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }

        // 副核在与主核相同的 satp 激活状态下进入调度循环
        // 注意：副核不需要重新设置 satp，主核激活 Sv39 后副核也运行在同一地址空间
        // （QEMU 多核共享 MMU，但每核有独立 satp CSR；副核 satp 仍为 0，
        //  即裸机物理地址模式——这对内核恒等映射来说是安全的）

        // 副核进入同一调度循环，使用 SCHED_LOCK 竞争任务
        sched_loop_secondary();
    }

    let layout = tg_linker::KernelLayout::locate();
    // 步骤 1：清零 BSS 段（未初始化全局变量区域）
    unsafe { layout.zero_bss() };
    BSS_READY.store(true, Ordering::Release);

    let need_secondaries = QEMU_SMP.saturating_sub(1);
    while SECONDARIES_ONLINE.load(Ordering::Acquire) < need_secondaries {
        core::hint::spin_loop();
    }

    // 步骤 2：初始化控制台输出和日志系统
    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG"));
    tg_console::test_log();
    // 步骤 3：初始化内核堆分配器
    // 堆起始地址为内核镜像起始处，可用区域为内核镜像结束处到物理内存末尾
    tg_kernel_alloc::init(layout.start() as _);
    unsafe {
        tg_kernel_alloc::transfer(core::slice::from_raw_parts_mut(
            layout.end() as _,
            MEMORY - layout.len(),
        ))
    };
    // 步骤 4：分配异界传送门所需的物理页面
    let portal_size = MultislotPortal::calculate_size(1);
    let portal_layout = Layout::from_size_align(portal_size, 1 << Sv39::PAGE_BITS).unwrap();
    let portal_ptr = unsafe { alloc(portal_layout) };
    assert!(portal_layout.size() < 1 << Sv39::PAGE_BITS);
    // 步骤 5：建立内核地址空间并激活 Sv39 分页
    kernel_space(layout, MEMORY, portal_ptr as _);
    // 步骤 6：初始化异界传送门（设置传送门页面的虚拟地址和 slot 数量）
    let portal = unsafe { MultislotPortal::init_transit(PROTAL_TRANSIT.base().val(), 1) };
    // 步骤 7：初始化系统调用处理器
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    tg_syscall::init_scheduling(&SyscallContext);
    tg_syscall::init_clock(&SyscallContext);
    tg_syscall::init_memory(&SyscallContext);
    // 步骤 8：加载初始进程 initproc
    // initproc 是所有用户进程的祖先，它会 fork 出 shell 进程
    let initproc_data = APPS.get("initproc").unwrap();
    if let Some(process) = Process::from_elf(ElfFile::new(initproc_data).unwrap()) {
        // 初始化进程管理器并添加 initproc
        PROCESSOR.get_mut().set_manager(ProcManager::new());
        PROCESSOR
            .get_mut()
            .add(process.pid, process, ProcId::from_usize(usize::MAX));
    }

    BOOT_DONE.store(true, Ordering::Release);

    // 通过 IPI 唤醒所有副核，让它们加入调度循环
    let secondary_mask: usize = ((1usize << QEMU_SMP) - 1) & !1;
    if secondary_mask != 0 {
        tg_sbi::send_ipi(secondary_mask);
    }

    // ─── 主调度循环（带 SCHED_LOCK 保护，支持多核并发调度）───
    run_sched_loop(portal)
}

/// 共享调度主函数（主核和副核均调用，通过 SCHED_LOCK 互斥）。
///
/// 每次迭代：持锁 → 取下一个就绪任务 → 执行 → 处理 Trap → 释放锁。
/// 无任务时短暂 wfi 等待（避免空转消耗总线带宽）。
#[cfg(target_arch = "riscv64")]
fn run_sched_loop(portal: &'static mut tg_kernel_context::foreign::MultislotPortal) -> ! {
    loop {
        // 持锁期间独占 PROCESSOR，保证 find_next/make_current 不会并发冲突
        let _guard = SCHED_LOCK.lock();

        let processor: *mut PManager<Process, ProcManager> = PROCESSOR.get_mut() as *mut _;
        if let Some(task) = unsafe { (*processor).find_next() } {
            // 通过异界传送门切换到用户地址空间执行用户程序
            unsafe { task.context.execute(portal, ()) };

            // ─── Trap 返回后处理（锁仍持有）───
            match scause::read().cause() {
                scause::Trap::Exception(scause::Exception::UserEnvCall) => {
                    use tg_syscall::{SyscallId as Id, SyscallResult as Ret};
                    let id: Id;
                    let args: [usize; 6];
                    {
                        let ctx = &mut task.context.context;
                        ctx.move_next();
                        id = ctx.a(7).into();
                        args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];
                    }
                    match tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args) {
                        Ret::Done(ret) => match id {
                            Id::EXIT => unsafe { (*processor).make_current_exited(ret) },
                            _ => {
                                const STRIDE_BIG: usize = 0x1000_0000;
                                *task.context.context.a_mut(0) = ret as _;
                                task.stride = task
                                    .stride
                                    .wrapping_add(STRIDE_BIG / task.priority.max(1));
                                unsafe { (*processor).make_current_suspend() };
                            }
                        },
                        Ret::Unsupported(_) => {
                            log::info!("id = {id:?}");
                            unsafe { (*processor).make_current_exited(-2) };
                        }
                    }
                }
                e => {
                    log::error!("unsupported trap: {e:?}");
                    unsafe { (*processor).make_current_exited(-3) };
                }
            }
            // _guard 在此处 drop，释放锁 → 其他核可立即抢入
        } else {
            // 无就绪任务：释放锁后短暂等待（wfi 让其他核有机会处理中断/唤醒进程）
            drop(_guard);
            // 检查是否真的没有任何进程存在（包含僵尸/睡眠）
            // 若进程全部退出，关机
            // 简化实现：直接关机；完整实现应检查所有进程是否均已退出
            println!("no task");
            tg_sbi::shutdown(false);
        }
    }
}

/// 副核调度入口：等待 BOOT_DONE 后进入共享调度循环。
///
/// 副核没有异界传送门的独立引用，但 `PROTAL_TRANSIT` 是编译期常量虚拟地址，
/// 在恒等映射下物理地址相同，所有核可以共用同一个 portal 实例。
#[cfg(target_arch = "riscv64")]
fn sched_loop_secondary() -> ! {
    // 副核重新从已知的虚拟地址获取 portal 引用（和主核使用同一个物理页）
    let portal = unsafe {
        tg_kernel_context::foreign::MultislotPortal::init_transit(PROTAL_TRANSIT.base().val(), 1)
    };
    // 副核同样需要激活 Sv39 分页，使用和主核相同的页表（通过 satp CSR）
    // 在 QEMU 多核下，主核设置的 satp 不会自动传播到副核，
    // 副核的 satp 仍为 0（裸机物理寻址）。
    // 由于内核采用恒等映射（VA==PA），副核在 satp=0 模式下访问内核数据是安全的。
    // 用户程序执行时，MultislotPortal 的 execute() 会自动切换 satp 到该进程的页表，
    // 并在 Trap 返回后恢复内核的 satp——这与单核行为一致。
    run_sched_loop(portal)
}

/// Rust panic 处理函数，打印错误信息并以异常方式关机
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    tg_sbi::shutdown(true)
}

/// 建立内核地址空间
///
/// 内核使用**恒等映射**（Identity Mapping）：虚拟地址 == 物理地址。
///
/// 映射内容：
/// 1. 内核代码段（.text）：可执行、可读、有效
/// 2. 只读数据段（.rodata）：可读、有效
/// 3. 数据段（.data）和启动段（.boot）：可写、可读、有效
/// 4. 堆区域：可写、可读、有效
/// 5. 异界传送门页面：全权限（内核和用户共享）
fn kernel_space(layout: tg_linker::KernelLayout, memory: usize, portal: usize) {
    let mut space = AddressSpace::new();
    // 映射内核各段（恒等映射：VPN == PPN）
    for region in layout.iter() {
        log::info!("{region}");
        use tg_linker::KernelRegionTitle::*;
        let flags = match region.title {
            Text => "X_RV",       // 代码段：可执行、可读
            Rodata => "__RV",     // 只读数据：可读
            Data | Boot => "_WRV", // 数据段：可写、可读
        };
        let s = VAddr::<Sv39>::new(region.range.start);
        let e = VAddr::<Sv39>::new(region.range.end);
        space.map_extern(
            s.floor()..e.ceil(),
            PPN::new(s.floor().val()),
            build_flags(flags),
        )
    }
    // 映射堆区域（内核镜像结束处到物理内存末尾）
    let s = VAddr::<Sv39>::new(layout.end());
    let e = VAddr::<Sv39>::new(layout.start() + memory);
    log::info!("(heap) ---> {:#10x}..{:#10x}", s.val(), e.val());
    space.map_extern(
        s.floor()..e.ceil(),
        PPN::new(s.floor().val()),
        build_flags("_WRV"),
    );
    // 映射异界传送门页面到虚拟地址空间最高页
    // 标志位 __G_XWRV：全局、可执行、可写、可读、有效
    space.map_extern(
        PROTAL_TRANSIT..PROTAL_TRANSIT + 1,
        PPN::new(portal >> Sv39::PAGE_BITS),
        build_flags("__G_XWRV"),
    );
    println!();
    // 激活 Sv39 分页模式：写入 satp 寄存器
    unsafe { satp::set(satp::Mode::Sv39, 0, space.root_ppn().val()) };
    // 保存内核地址空间到全局变量
    unsafe { KERNEL_SPACE.write(space) };
}

/// 将内核地址空间中的异界传送门页表项复制到用户地址空间
///
/// 这确保了内核和用户地址空间在传送门虚拟地址处映射到同一物理页面，
/// 使得切换 satp 时代码仍然可以正常执行。
fn map_portal(space: &AddressSpace<Sv39, Sv39Manager>) {
    let portal_idx = PROTAL_TRANSIT.index_in(Sv39::MAX_LEVEL);
    space.root()[portal_idx] = unsafe { KERNEL_SPACE.assume_init_ref() }.root()[portal_idx];
}

/// 各种接口库的实现
///
/// 本模块为 tg-syscall 提供的各个 trait 提供具体实现，
/// 包括 IO、Process、Scheduling、Clock、Memory 等系统调用接口。
mod impls {
    use crate::{
        build_flags, parse_flags, process::Process as ProcStruct, processor::ProcManager, Sv39,
        APPS, PROCESSOR,
    };
    use alloc::alloc::alloc_zeroed;
    use core::{alloc::Layout, ptr::NonNull};
    use tg_console::log;
    use tg_kernel_vm::{
        page_table::{MmuMeta, Pte, VAddr, VmFlags, PPN, VPN},
        PageManager,
    };
    use tg_syscall::*;
    use tg_task_manage::{PManager, ProcId};
    use xmas_elf::ElfFile;

    // ─── Sv39 页表管理器 ───

    /// Sv39 页表管理器
    ///
    /// 实现 `PageManager<Sv39>` trait，负责：
    /// - 物理页面的分配和释放
    /// - 物理地址与虚拟地址的转换（恒等映射下两者相等）
    /// - 页面所有权标记（OWNED 标志位）
    #[repr(transparent)]
    pub struct Sv39Manager(NonNull<Pte<Sv39>>);

    impl Sv39Manager {
        /// 自定义标志位：标记此页面由内核分配（用于区分恒等映射的外部页面）
        const OWNED: VmFlags<Sv39> = unsafe { VmFlags::from_raw(1 << 8) };

        /// 分配对齐的物理页面（已清零）
        #[inline]
        fn page_alloc<T>(count: usize) -> *mut T {
            unsafe {
                alloc_zeroed(Layout::from_size_align_unchecked(
                    count << Sv39::PAGE_BITS,
                    1 << Sv39::PAGE_BITS,
                ))
            }
            .cast()
        }
    }

    impl PageManager<Sv39> for Sv39Manager {
        /// 创建新的根页表
        #[inline]
        fn new_root() -> Self {
            Self(NonNull::new(Self::page_alloc(1)).unwrap())
        }

        /// 获取根页表的物理页号
        #[inline]
        fn root_ppn(&self) -> PPN<Sv39> {
            PPN::new(self.0.as_ptr() as usize >> Sv39::PAGE_BITS)
        }

        /// 获取根页表的指针
        #[inline]
        fn root_ptr(&self) -> NonNull<Pte<Sv39>> {
            self.0
        }

        /// 物理页号转虚拟地址（恒等映射下直接转换）
        #[inline]
        fn p_to_v<T>(&self, ppn: PPN<Sv39>) -> NonNull<T> {
            unsafe { NonNull::new_unchecked(VPN::<Sv39>::new(ppn.val()).base().as_mut_ptr()) }
        }

        /// 虚拟地址转物理页号（恒等映射下直接转换）
        #[inline]
        fn v_to_p<T>(&self, ptr: NonNull<T>) -> PPN<Sv39> {
            PPN::new(VAddr::<Sv39>::new(ptr.as_ptr() as _).floor().val())
        }

        /// 检查页表项是否由内核分配
        #[inline]
        fn check_owned(&self, pte: Pte<Sv39>) -> bool {
            pte.flags().contains(Self::OWNED)
        }

        /// 分配物理页面并标记为内核所有
        #[inline]
        fn allocate(&mut self, len: usize, flags: &mut VmFlags<Sv39>) -> NonNull<u8> {
            *flags |= Self::OWNED;
            NonNull::new(Self::page_alloc(len)).unwrap()
        }

        fn deallocate(&mut self, _pte: Pte<Sv39>, _len: usize) -> usize {
            todo!()
        }

        fn drop_root(&mut self) {
            todo!()
        }
    }

    // ─── 控制台实现 ───

    /// 控制台输出实现，通过 SBI 接口逐字符输出
    pub struct Console;

    impl tg_console::Console for Console {
        #[inline]
        fn put_char(&self, c: u8) {
            tg_sbi::console_putchar(c);
        }
    }

    // ─── 系统调用实现 ───

    /// 系统调用上下文，实现 IO、Process、Scheduling、Clock、Memory 等 trait
    pub struct SyscallContext;

    /// IO 系统调用实现：write 和 read
    impl IO for SyscallContext {
        /// write 系统调用：将数据写入标准输出
        ///
        /// 需要通过 `translate()` 将用户虚拟地址翻译为物理地址，
        /// 并检查可读权限后才能访问用户缓冲区。
        fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            match fd {
                STDOUT | STDDEBUG => {
                    const READABLE: VmFlags<Sv39> = build_flags("RV");
                    if let Some(ptr) = PROCESSOR
                        .get_mut()
                        .current()
                        .unwrap()
                        .address_space
                        .translate::<u8>(VAddr::new(buf), READABLE)
                    {
                        print!("{}", unsafe {
                            core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                                ptr.as_ptr(),
                                count,
                            ))
                        });
                        count as _
                    } else {
                        log::error!("ptr not readable");
                        -1
                    }
                }
                _ => {
                    log::error!("unsupported fd: {fd}");
                    -1
                }
            }
        }

        /// read 系统调用：从标准输入读取数据
        ///
        /// 通过 SBI console_getchar 接口逐字符读取，
        /// 同样需要地址翻译和可写权限检查。
        #[inline]
        fn read(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            if fd == STDIN {
                const WRITEABLE: VmFlags<Sv39> = build_flags("W_V");
                if let Some(mut ptr) = PROCESSOR
                    .get_mut()
                    .current()
                    .unwrap()
                    .address_space
                    .translate::<u8>(VAddr::new(buf), WRITEABLE)
                {
                    let mut ptr = unsafe { ptr.as_mut() } as *mut u8;
                    for _ in 0..count {
                        let c = tg_sbi::console_getchar() as u8;
                        unsafe {
                            *ptr = c;
                            ptr = ptr.add(1);
                        }
                    }
                    count as _
                } else {
                    log::error!("ptr not writeable");
                    -1
                }
            } else {
                log::error!("unsupported fd: {fd}");
                -1
            }
        }
    }

    /// 进程管理系统调用实现
    impl Process for SyscallContext {
        /// exit 系统调用：退出当前进程
        ///
        /// 返回 exit_code，由 ProcManager 标记为僵尸状态，
        /// 等待父进程通过 wait 回收。
        #[inline]
        fn exit(&self, _caller: Caller, exit_code: usize) -> isize {
            exit_code as isize
        }

        /// fork 系统调用：创建子进程
        ///
        /// 复制父进程的完整地址空间（深拷贝页表和物理页面），
        /// 父进程返回子进程 PID，子进程返回 0。
        fn fork(&self, _caller: Caller) -> isize {
            let processor: *mut PManager<ProcStruct, ProcManager> = PROCESSOR.get_mut() as *mut _;
            let current = unsafe { (*processor).current().unwrap() };
            let parent_pid = current.pid; // 保存父进程 PID
            let mut child_proc = current.fork().unwrap();
            let pid = child_proc.pid;
            let context = &mut child_proc.context.context;
            // 子进程的 a0 寄存器设为 0（fork 的返回值）
            *context.a_mut(0) = 0 as _;
            // 将子进程加入进程管理器，父进程 PID 用于维护进程树
            unsafe { (*processor).add(pid, child_proc, parent_pid) };
            // 父进程返回子进程 PID
            pid.get_usize() as isize
        }

        /// exec 系统调用：加载并执行新程序
        ///
        /// 根据用户传入的程序名（需地址翻译），查找对应的 ELF 数据，
        /// 替换当前进程的地址空间。
        fn exec(&self, _caller: Caller, path: usize, count: usize) -> isize {
            const READABLE: VmFlags<Sv39> = build_flags("RV");
            let current = PROCESSOR.get_mut().current().unwrap();
            current
                .address_space
                .translate::<u8>(VAddr::new(path), READABLE)
                .map(|ptr| unsafe {
                    core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr.as_ptr(), count))
                })
                .and_then(|name| APPS.get(name))
                .and_then(|input| ElfFile::new(input).ok())
                .map_or_else(
                    || {
                        log::error!("unknown app, select one in the list: ");
                        APPS.keys().for_each(|app| println!("{app}"));
                        println!();
                        -1
                    },
                    |data| {
                        current.exec(data);
                        0
                    },
                )
        }

        /// wait 系统调用：等待子进程退出
        ///
        /// - pid == -1：等待任意子进程
        /// - pid > 0：等待指定 PID 的子进程
        /// 返回值：成功返回子进程 PID，无子进程返回 -1
        fn wait(&self, _caller: Caller, pid: isize, exit_code_ptr: usize) -> isize {
            let processor: *mut PManager<ProcStruct, ProcManager> = PROCESSOR.get_mut() as *mut _;
            let current = unsafe { (*processor).current().unwrap() };
            const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
            if let Some((dead_pid, exit_code)) =
                unsafe { (*processor).wait(ProcId::from_usize(pid as usize)) }
            {
                // 将退出码写入用户空间指针（需地址翻译）
                if let Some(mut ptr) = current
                    .address_space
                    .translate::<i32>(VAddr::new(exit_code_ptr), WRITABLE)
                {
                    unsafe { *ptr.as_mut() = exit_code as i32 };
                }
                return dead_pid.get_usize() as isize;
            } else {
                // 等待的子进程不存在
                return -1;
            }
        }

        /// getpid 系统调用：获取当前进程 PID
        fn getpid(&self, _caller: Caller) -> isize {
            let current = PROCESSOR.get_mut().current().unwrap();
            current.pid.get_usize() as _
        }

        /// spawn 系统调用：创建新进程并直接执行指定程序
        ///
        /// 与 fork+exec 不同，spawn 直接从 ELF 创建新进程，
        /// 无需复制父进程地址空间。
        fn spawn(&self, _caller: Caller, path: usize, count: usize) -> isize {
            const READABLE: VmFlags<Sv39> = build_flags("RV");
            let processor: *mut PManager<ProcStruct, ProcManager> = PROCESSOR.get_mut() as *mut _;
            let current = unsafe { (*processor).current().unwrap() };
            let parent_pid = current.pid;
            current
                .address_space
                .translate::<u8>(VAddr::new(path), READABLE)
                .map(|ptr| unsafe {
                    core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr.as_ptr(), count))
                })
                .and_then(|name| APPS.get(name))
                .and_then(|input| ElfFile::new(input).ok())
                .and_then(ProcStruct::from_elf)
                .map(|child| {
                    let pid = child.pid;
                    unsafe { (*processor).add(pid, child, parent_pid) };
                    pid.get_usize() as isize
                })
                .unwrap_or(-1)
        }

        /// sbrk 系统调用：调整进程堆空间大小
        ///
        /// - size > 0：扩展堆
        /// - size < 0：收缩堆
        /// - size == 0：返回当前堆顶地址
        fn sbrk(&self, _caller: Caller, size: i32) -> isize {
            let current = PROCESSOR.get_mut().current().unwrap();
            if let Some(old_brk) = current.change_program_brk(size as isize) {
                old_brk as isize
            } else {
                -1
            }
        }
    }

    /// 调度系统调用实现
    impl Scheduling for SyscallContext {
        /// sched_yield 系统调用：主动让出 CPU
        #[inline]
        fn sched_yield(&self, _caller: Caller) -> isize {
            0
        }

        /// set_priority 系统调用：设置当前进程优先级
        fn set_priority(&self, _caller: Caller, prio: isize) -> isize {
            if prio < 2 {
                return -1;
            }
            let current = PROCESSOR.get_mut().current().unwrap();
            current.priority = prio as usize;
            prio
        }
    }

    /// 时钟系统调用实现
    impl Clock for SyscallContext {
        /// clock_gettime 系统调用：获取系统时间
        ///
        /// 读取 RISC-V time 寄存器，转换为纳秒级时间戳，
        /// 通过地址翻译写入用户空间的 TimeSpec 结构。
        #[inline]
        fn clock_gettime(&self, _caller: Caller, clock_id: ClockId, tp: usize) -> isize {
            const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
            match clock_id {
                ClockId::CLOCK_MONOTONIC => {
                    if let Some(mut ptr) = PROCESSOR
                        .get_mut()
                        .current()
                        .unwrap()
                        .address_space
                        .translate::<TimeSpec>(VAddr::new(tp), WRITABLE)
                    {
                        let time = riscv::register::time::read() * 10000 / 125;
                        *unsafe { ptr.as_mut() } = TimeSpec {
                            tv_sec: time / 1_000_000_000,
                            tv_nsec: time % 1_000_000_000,
                        };
                        0
                    } else {
                        log::error!("ptr not readable");
                        -1
                    }
                }
                _ => -1,
            }
        }
    }

    /// 内存管理系统调用实现
    impl Memory for SyscallContext {
        fn mmap(
            &self,
            _caller: Caller,
            addr: usize,
            len: usize,
            prot: i32,
            _flags: i32,
            _fd: i32,
            _offset: usize,
        ) -> isize {
            const PAGE: usize = 1 << Sv39::PAGE_BITS;
            if addr & (PAGE - 1) != 0 || addr >> 38 != 0 {
                return -1;
            }
            let Ok(vm_flags) = mmap_prot_to_flags(prot) else {
                return -1;
            };
            let alen = if len == 0 {
                0usize
            } else {
                (len + PAGE - 1) / PAGE * PAGE
            };
            let Some(end_v) = addr.checked_add(alen) else {
                return -1;
            };
            if end_v >> 38 != 0 {
                return -1;
            }
            let vpn_s = VAddr::new(addr).floor();
            let vpn_e = VAddr::new(end_v).ceil();
            if vpn_s >= vpn_e {
                return 0;
            }
            let current = PROCESSOR.get_mut().current().unwrap();
            const READABLE: VmFlags<Sv39> = build_flags("RV");
            let mut v = vpn_s;
            while v < vpn_e {
                if current
                    .address_space
                    .translate::<u8>(v.base(), READABLE)
                    .is_some()
                {
                    return -1;
                }
                v = v + 1;
            }
            current
                .address_space
                .map(vpn_s..vpn_e, &[], 0, vm_flags);
            0
        }

        fn munmap(&self, _caller: Caller, addr: usize, len: usize) -> isize {
            const PAGE: usize = 1 << Sv39::PAGE_BITS;
            if addr & (PAGE - 1) != 0 || addr >> 38 != 0 {
                return -1;
            }
            let alen = if len == 0 {
                0usize
            } else {
                (len + PAGE - 1) / PAGE * PAGE
            };
            let Some(end_v) = addr.checked_add(alen) else {
                return -1;
            };
            if end_v >> 38 != 0 {
                return -1;
            }
            let vpn_s = VAddr::new(addr).floor();
            let vpn_e = VAddr::new(end_v).ceil();
            if vpn_s >= vpn_e {
                return 0;
            }
            let current = PROCESSOR.get_mut().current().unwrap();
            const READABLE: VmFlags<Sv39> = build_flags("RV");
            let mut v = vpn_s;
            while v < vpn_e {
                if current
                    .address_space
                    .translate::<u8>(v.base(), READABLE)
                    .is_none()
                {
                    return -1;
                }
                v = v + 1;
            }
            current.address_space.unmap(vpn_s..vpn_e);
            0
        }
    }

    fn mmap_prot_to_flags(prot: i32) -> Result<VmFlags<Sv39>, ()> {
        if prot & !0x7 != 0 || prot & 0x7 == 0 {
            return Err(());
        }
        let mut f = *b"U___V";
        if prot & 1 != 0 {
            f[1] = b'R';
        }
        if prot & 2 != 0 {
            f[2] = b'W';
        }
        if prot & 4 != 0 {
            f[3] = b'X';
        }
        parse_flags(unsafe { core::str::from_utf8_unchecked(&f) })
    }
}

/// 非 RISC-V64 架构的占位实现
///
/// 在主机平台（如 x86_64）上提供占位符，使代码可以通过编译检查，
/// 方便在 IDE 中进行开发和语法检查。
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    use tg_kernel_vm::page_table::{MmuMeta, VmFlags};

    /// Sv39 占位类型（仅用于主机平台编译检查）
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
    pub struct Sv39;

    impl MmuMeta for Sv39 {
        const P_ADDR_BITS: usize = 56;
        const PAGE_BITS: usize = 12;
        const LEVEL_BITS: &'static [usize] = &[9, 9, 9];
        const PPN_POS: usize = 10;

        #[inline]
        fn is_leaf(value: usize) -> bool {
            value & 0b1110 != 0
        }
    }

    /// 构建 VmFlags 占位实现
    pub const fn build_flags(_s: &str) -> VmFlags<Sv39> {
        unsafe { VmFlags::from_raw(0) }
    }

    /// 解析 VmFlags 占位实现
    pub fn parse_flags(_s: &str) -> Result<VmFlags<Sv39>, ()> {
        Ok(unsafe { VmFlags::from_raw(0) })
    }

    /// 主机平台占位入口
    #[unsafe(no_mangle)]
    pub extern "C" fn main() -> i32 {
        0
    }

    /// libc 启动占位
    #[unsafe(no_mangle)]
    pub extern "C" fn __libc_start_main() -> i32 {
        0
    }

    /// 异常处理占位
    #[unsafe(no_mangle)]
    pub extern "C" fn rust_eh_personality() {}
}

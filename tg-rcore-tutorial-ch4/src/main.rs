//! # 第四章：地址空间
//!
//! 本章在第三章"多道程序与分时多任务"的基础上，引入了 **RISC-V Sv39 虚拟内存机制**，
//! 为每个用户进程提供独立的地址空间，实现进程间的内存隔离。
//!
//! ## 核心概念
//!
//! - **虚拟内存**：通过 Sv39 三级页表将虚拟地址映射到物理地址
//! - **地址空间隔离**：每个进程拥有独立的页表，无法访问其他进程的内存
//! - **异界传送门（MultislotPortal）**：解决跨地址空间的上下文切换问题
//! - **ELF 加载**：解析 ELF 格式的用户程序并映射到独立地址空间
//! - **内核堆分配器**：支持动态内存分配（`alloc` crate）
//! - **地址翻译**：系统调用中需要将用户虚拟地址翻译为物理地址
//!
//! 教程阅读建议：
//!
//! - 先看 `kernel_space`：建立“内核恒等映射 + 传送门映射”的总体视图；
//! - 再看 `schedule`：理解跨地址空间执行与 trap 返回路径；
//! - 最后看 `impls`：重点掌握 `translate()` 如何保证用户指针访问安全。

// 不使用标准库，裸机环境没有操作系统提供系统调用支持
#![no_std]
// 不使用标准入口，裸机环境没有 C runtime 进行初始化
#![no_main]
// RISC-V64 架构下启用严格警告和文档检查
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
// 非 RISC-V64 架构允许死代码和未使用导入（用于 cargo publish --dry-run）
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code, unused_imports))]

// 进程管理模块：定义 Process 结构体，包含地址空间和上下文
mod process;

// 引入控制台输出宏（print! / println!），由 tg_console 库提供
#[macro_use]
extern crate tg_console;

// 启用 alloc crate，提供堆分配能力（Vec、Box 等）
extern crate alloc;

// ========== 导入 ==========

use crate::{
    impls::{Sv39Manager, SyscallContext},
    process::Process,
};
use alloc::{alloc::alloc, vec::Vec};
use core::{alloc::Layout, cell::UnsafeCell};
use impls::Console;
use riscv::register::*;
// 非 RISC-V64 使用占位 Sv39 类型
#[cfg(not(target_arch = "riscv64"))]
use stub::Sv39;
use tg_console::log;
// 异界传送门：解决跨地址空间上下文切换的核心组件
use tg_kernel_context::{foreign::MultislotPortal, LocalContext};
// RISC-V64 使用真正的 Sv39 类型
#[cfg(target_arch = "riscv64")]
use tg_kernel_vm::page_table::Sv39;
use tg_kernel_vm::{
    page_table::{MmuMeta, VAddr, VmFlags, VmMeta, PPN, VPN},
    AddressSpace,
};
use tg_sbi;
use tg_syscall::Caller;
use xmas_elf::ElfFile;
#[cfg(target_arch = "riscv64")]
include!(concat!(env!("OUT_DIR"), "/qemu_smp.rs"));
#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ========== 辅助函数 ==========

/// 从字符串构建页表项标志位（编译期常量）。
///
/// 字符串格式如 `"U_WRV"` 表示 User + Write + Read + Valid。
#[cfg(target_arch = "riscv64")]
const fn build_flags(s: &str) -> VmFlags<Sv39> {
    VmFlags::build_from_str(s)
}

/// 从字符串解析页表项标志位（运行期）。
#[cfg(target_arch = "riscv64")]
fn parse_flags(s: &str) -> Result<VmFlags<Sv39>, ()> {
    s.parse()
}

// 非 RISC-V64 架构使用占位实现
#[cfg(not(target_arch = "riscv64"))]
use stub::{build_flags, parse_flags};

// ========== 启动相关 ==========

// 将用户程序的二进制数据内联到内核镜像中
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!(env!("APP_ASM")));

/// hart 0 完成 `zero_bss` 后置位；副核在此之前不得访问 BSS 内同步变量（实验 10）。
#[cfg(target_arch = "riscv64")]
static BSS_READY: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "riscv64")]
static CONSOLE_LOCK: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "riscv64")]
static SECONDARIES_ONLINE: AtomicUsize = AtomicUsize::new(0);

/// hart 0 即将进入调度线程前置位（实验 10）。
#[cfg(target_arch = "riscv64")]
static BOOT_DONE: AtomicBool = AtomicBool::new(false);

/// 多核引导栈槽数量（与 `tg-sbi` M 态一致）。
#[cfg(target_arch = "riscv64")]
const SMP_HART_MAX: usize = 8;

// 定义内核入口点：分配 24 KiB 内核栈。
//
// 这里不再调用 tg_linker::boot0! 宏，避免外部已发布版本与 Rust 2024
// 在属性语义上的兼容差异影响本 crate 的发布校验。
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = 6 * 4096;
    #[unsafe(link_section = ".boot.stack")]
    static mut STACKS: [[u8; STACK_SIZE]; SMP_HART_MAX] =
        [[0u8; STACK_SIZE]; SMP_HART_MAX];

    core::arch::naked_asm!(
        "li     t1, {max_harts}",
        "bgeu   a0, t1, 9f",
        "la     t2, {stacks}",
        "slli   t3, a0, 14",
        "slli   t4, a0, 13",
        "add    t3, t3, t4",
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

// 物理内存容量 = 24 MiB（QEMU virt 平台的 RAM 大小）
const MEMORY: usize = 24 << 20;

// 异界传送门所在虚页：虚拟地址空间的最高页
// 传送门同时映射到内核和所有用户地址空间的相同虚拟地址，
// 使得切换 satp（地址空间）后代码仍然可以执行
const PROTAL_TRANSIT: VPN<Sv39> = VPN::MAX;

// ========== 进程列表 ==========

/// 全局进程列表（用 UnsafeCell 包装以允许内部可变性）。
struct ProcessList(UnsafeCell<Vec<Process>>);

unsafe impl Sync for ProcessList {}

impl ProcessList {
    const fn new() -> Self {
        Self(UnsafeCell::new(Vec::new()))
    }

    unsafe fn get_mut(&self) -> &mut Vec<Process> {
        unsafe { &mut *self.0.get() }
    }
}

/// 全局进程列表实例。
static PROCESSES: ProcessList = ProcessList::new();

// ========== 内核主函数 ==========

/// 内核主函数：初始化各子系统，建立内核地址空间，加载用户进程。
///
/// 与前几章不同，本章需要：
/// 1. 初始化内核堆（支持动态分配）
/// 2. 建立异界传送门（跨地址空间切换）
/// 3. 建立内核地址空间（Sv39 页表）
/// 4. 为每个用户程序解析 ELF 并创建独立地址空间
/// 5. 建立调度线程执行用户进程
///
/// `hartid` 由 `tg-sbi` 经 `a0` 传入；仅 hart 0 跑调度线程；副核经 `BSS_READY` 后打印上线行再 `wfi`（实验 10）。
#[cfg(target_arch = "riscv64")]
extern "C" fn rust_main(hartid: usize) -> ! {
    if hartid != 0 {
        while !BSS_READY.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        use tg_sbi::console_putchar;
        with_console_lock(|| {
            for c in b"[ch4] hart " {
                console_putchar(*c);
            }
            put_decimal_hart(hartid);
            for c in b" secondary online, waiting for scheduler\n" {
                console_putchar(*c);
            }
        });
        SECONDARIES_ONLINE.fetch_add(1, Ordering::Release);
        // ch4 调度线程运行在主核异常域中（LocalContext::thread），
        // 副核实现完整多核调度需要额外的调度线程机制，留作后续扩展。
        // 当前：副核等待 BOOT_DONE 后进入 wfi 待命。
        while !BOOT_DONE.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }
        loop {
            unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
        }
    }

    let layout = tg_linker::KernelLayout::locate();
    // 第一步：清零 BSS 段
    unsafe { layout.zero_bss() };
    BSS_READY.store(true, Ordering::Release);

    let need_secondaries = QEMU_SMP.saturating_sub(1);
    while SECONDARIES_ONLINE.load(Ordering::Acquire) < need_secondaries {
        core::hint::spin_loop();
    }

    // 第二步：初始化控制台
    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG"));
    tg_console::test_log();
    // 第三步：初始化内核堆分配器
    // 堆的起始地址为内核镜像起始处，可用内存为内核镜像之后到物理内存末尾
    tg_kernel_alloc::init(layout.start() as _);
    unsafe {
        tg_kernel_alloc::transfer(core::slice::from_raw_parts_mut(
            layout.end() as _,
            MEMORY - layout.len(),
        ))
    };
    // 第四步：分配异界传送门的物理页面
    // 传送门大小需要适配 1 个 slot（对应 1 个并发切换）
    let portal_size = MultislotPortal::calculate_size(1);
    let portal_layout = Layout::from_size_align(portal_size, 1 << Sv39::PAGE_BITS).unwrap();
    let portal_ptr = unsafe { alloc(portal_layout) };
    assert!(portal_layout.size() < 1 << Sv39::PAGE_BITS);
    // 第五步：建立内核地址空间（恒等映射 + 传送门映射）
    let mut ks = kernel_space(layout, MEMORY, portal_ptr as _);
    let portal_idx = PROTAL_TRANSIT.index_in(Sv39::MAX_LEVEL);
    // 第六步：加载用户程序
    // 解析每个 ELF 文件，创建独立地址空间，映射传送门
    for (i, elf) in tg_linker::AppMeta::locate().iter().enumerate() {
        let base = elf.as_ptr() as usize;
        log::info!("detect app[{i}]: {base:#x}..{:#x}", base + elf.len());
        if let Some(process) = Process::new(ElfFile::new(elf).unwrap()) {
            // 将内核传送门页表项共享到用户地址空间
            // 这样传送门在两个地址空间的虚拟地址相同
            process.address_space.root()[portal_idx] = ks.root()[portal_idx];
            unsafe { PROCESSES.get_mut().push(process) };
        }
    }

    // 第七步：建立调度栈（映射到内核地址空间的高地址区域）
    // syscall 计数等使 schedule 路径栈帧变大，须分配与映射页数一致
    let pages = 4;
    let stack_layout = Layout::from_size_align(pages * (1 << Sv39::PAGE_BITS), 1 << Sv39::PAGE_BITS)
        .unwrap();
    let stack = unsafe { alloc(stack_layout) };
    ks.map_extern(
        VPN::new((1 << 26) - pages)..VPN::new(1 << 26),
        PPN::new(stack as usize >> Sv39::PAGE_BITS),
        build_flags("_WRV"),
    );
    // 第八步：建立调度线程
    // 调度线程在独立的异常域运行，内核异常不会导致整个系统崩溃
    let mut scheduling = LocalContext::thread(schedule as *const () as _, false);
    *scheduling.sp_mut() = 1 << 38;
    BOOT_DONE.store(true, Ordering::Release);
    // 通知副核系统已就绪（副核收到 IPI 后从 spin_loop 退出，进入 wfi 待命）
    let secondary_mask: usize = ((1usize << QEMU_SMP) - 1) & !1;
    if secondary_mask != 0 {
        tg_sbi::send_ipi(secondary_mask);
    }
    unsafe { scheduling.execute() };
    // 如果从 execute() 返回，说明调度线程发生了异常
    log::error!("stval = {:#x}", stval::read());
    panic!("trap from scheduling thread: {:?}", scause::read().cause());
}

// ========== 调度函数 ==========

/// 调度函数：在异界传送门中循环执行所有用户进程。
///
/// 工作流程：
/// 1. 初始化传送门和系统调用
/// 2. 取出第一个进程，通过传送门切换到其地址空间并执行
/// 3. Trap 返回后处理系统调用或异常
/// 4. 进程退出后从列表中移除，继续下一个
extern "C" fn schedule() -> ! {
    // 初始化异界传送门（设置传送门页面的虚拟地址和 slot 数量）
    let portal = unsafe { MultislotPortal::init_transit(PROTAL_TRANSIT.base().val(), 1) };
    // 初始化系统调用处理
    // 比第三章多了 memory（mmap/munmap/sbrk）
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    tg_syscall::init_scheduling(&SyscallContext);
    tg_syscall::init_clock(&SyscallContext);
    tg_syscall::init_trace(&SyscallContext);
    tg_syscall::init_memory(&SyscallContext);

    // 调度循环：持续执行直到所有进程完成
    while !unsafe { PROCESSES.get_mut().is_empty() } {
        let processes = unsafe { PROCESSES.get_mut() };
        let proc = &mut processes[0];
        // 通过传送门执行用户进程：
        // 1. 跳转到传送门页面
        // 2. 在传送门内切换 satp 到用户地址空间
        // 3. 恢复用户寄存器，执行 sret 进入 U-mode
        // 4. 用户触发 Trap 后，传送门切换回内核地址空间
        unsafe { proc.context.execute(portal, ()) };

        // 处理 Trap
        match scause::read().cause() {
            // ─── 系统调用 ───
            scause::Trap::Exception(scause::Exception::UserEnvCall) => {
                use crate::process::{self, SYSCALL_COUNT_LEN};
                use tg_syscall::{SyscallId as Id, SyscallResult as Ret};

                let ctx = &mut proc.context.context;
                let id: Id = ctx.a(7).into();
                let args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];
                if id.0 < SYSCALL_COUNT_LEN {
                    proc.syscall_counts[id.0] = proc.syscall_counts[id.0].saturating_add(1);
                }
                process::set_active_syscall_counts(&mut proc.syscall_counts);
                let handle_res = tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args);
                process::clear_active_syscall_counts();
                match handle_res {
                    Ret::Done(ret) => match id {
                        // exit：移除进程
                        Id::EXIT => unsafe {
                            PROCESSES.get_mut().remove(0);
                        },
                        // 其他系统调用：写回返回值，sepc += 4
                        _ => {
                            *ctx.a_mut(0) = ret as _;
                            ctx.move_next();
                        }
                    },
                    // 不支持的系统调用：杀死进程
                    Ret::Unsupported(_) => {
                        log::info!("id = {id:?}");
                        unsafe { PROCESSES.get_mut().remove(0) };
                    }
                }
            }
            // ─── 其他异常/中断：杀死进程 ───
            e => {
                log::error!(
                    "unsupported trap: {e:?}, stval = {:#x}, sepc = {:#x}",
                    stval::read(),
                    proc.context.context.pc()
                );
                unsafe { PROCESSES.get_mut().remove(0) };
            }
        }
    }
    // 所有进程执行完毕，关机
    tg_sbi::shutdown(false)
}

// ========== panic 处理 ==========

/// panic 处理函数：打印错误信息后以异常状态关机。
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::error!("{info}");
    tg_sbi::shutdown(true)
}

// ========== 内核地址空间构建 ==========

/// 建立内核地址空间。
///
/// 包含以下映射：
/// - **恒等映射**（Identity Mapping）：内核代码段、数据段、堆区域
///   虚拟地址 == 物理地址，方便内核直接访问物理内存
/// - **传送门映射**：将传送门物理页映射到虚拟地址空间最高页
fn kernel_space(
    layout: tg_linker::KernelLayout,
    memory: usize,
    portal: usize,
) -> AddressSpace<Sv39, Sv39Manager> {
    let mut space = AddressSpace::<Sv39, Sv39Manager>::new();
    // 映射内核各段（恒等映射：VPN == PPN）
    for region in layout.iter() {
        log::info!("{region}");
        use tg_linker::KernelRegionTitle::*;
        let flags = match region.title {
            Text => "X_RV",    // 代码段：可执行、可读
            Rodata => "__RV",  // 只读数据段：只读
            Data | Boot => "_WRV", // 数据段/启动段：可读写
        };
        let s = VAddr::<Sv39>::new(region.range.start);
        let e = VAddr::<Sv39>::new(region.range.end);
        space.map_extern(
            s.floor()..e.ceil(),
            PPN::new(s.floor().val()),
            build_flags(flags),
        )
    }
    // 映射内核堆区域（恒等映射）
    log::info!(
        "(heap) ---> {:#10x}..{:#10x}",
        layout.end(),
        layout.start() + memory
    );
    let s = VAddr::<Sv39>::new(layout.end());
    let e = VAddr::<Sv39>::new(layout.start() + memory);
    space.map_extern(
        s.floor()..e.ceil(),
        PPN::new(s.floor().val()),
        build_flags("_WRV"),
    );
    // 映射异界传送门到虚拟地址空间最高页
    // 标志位 "__G_XWRV" 表示全局、可执行、可读写、有效
    space.map_extern(
        PROTAL_TRANSIT..PROTAL_TRANSIT + 1,
        PPN::new(portal >> Sv39::PAGE_BITS),
        build_flags("__G_XWRV"),
    );
    println!();
    // 激活内核地址空间：写入 satp 寄存器，开启 Sv39 分页模式
    unsafe { satp::set(satp::Mode::Sv39, 0, space.root_ppn().val()) };
    space
}

// ========== 接口实现 ==========

/// 各依赖库所需接口的具体实现。
///
/// 与前几章不同，本章的系统调用实现需要进行**地址翻译**：
/// 用户传入的指针是虚拟地址，内核需要通过页表将其翻译为物理地址才能访问。
mod impls {
    use crate::{build_flags, parse_flags, process, Sv39, PROCESSES};
    use alloc::alloc::alloc_zeroed;
    use core::{alloc::Layout, ptr::NonNull};
    use tg_console::log;
    use tg_kernel_vm::{
        page_table::{MmuMeta, Pte, VAddr, VmFlags, PPN, VPN},
        PageManager,
    };
    use tg_syscall::*;

    /// Sv39 页表管理器：负责物理页的分配和映射。
    #[repr(transparent)]
    pub struct Sv39Manager(NonNull<Pte<Sv39>>);

    impl Sv39Manager {
        /// 自定义标志位：标记该页面由内核分配（用于 deallocate 时判断）
        const OWNED: VmFlags<Sv39> = unsafe { VmFlags::from_raw(1 << 8) };

        /// 分配物理页面并清零
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

    /// 实现 PageManager trait：为地址空间提供页表操作能力
    impl PageManager<Sv39> for Sv39Manager {
        /// 创建新的根页表（分配一个物理页）
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

        /// 物理页号 → 虚拟地址指针（恒等映射下 PPN == VPN）
        #[inline]
        fn p_to_v<T>(&self, ppn: PPN<Sv39>) -> NonNull<T> {
            unsafe { NonNull::new_unchecked(VPN::<Sv39>::new(ppn.val()).base().as_mut_ptr()) }
        }

        /// 虚拟地址指针 → 物理页号
        #[inline]
        fn v_to_p<T>(&self, ptr: NonNull<T>) -> PPN<Sv39> {
            PPN::new(VAddr::<Sv39>::new(ptr.as_ptr() as _).floor().val())
        }

        /// 检查页表项是否由内核分配
        #[inline]
        fn check_owned(&self, pte: Pte<Sv39>) -> bool {
            pte.flags().contains(Self::OWNED)
        }

        /// 分配物理页面：清零并标记为内核拥有
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

    /// IO 系统调用实现
    ///
    /// **与前几章的关键区别**：用户传入的 `buf` 是虚拟地址，
    /// 需要通过 `address_space.translate()` 翻译为物理地址才能访问。
    impl IO for SyscallContext {
        fn write(&self, caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            match fd {
                STDOUT | STDDEBUG => {
                    // 检查用户地址是否可读
                    const READABLE: VmFlags<Sv39> = build_flags("RV");
                    if let Some(ptr) = unsafe { PROCESSES.get_mut() }
                        .get_mut(caller.entity)
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
    }

    /// Process 系统调用实现
    impl Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: Caller, _status: usize) -> isize {
            0
        }

        /// sbrk：调整进程堆空间大小
        ///
        /// 这是本章新增的系统调用，允许用户程序动态扩展/收缩堆内存。
        /// 返回旧的 break 地址，失败返回 -1。
        fn sbrk(&self, caller: Caller, size: i32) -> isize {
            if let Some(process) = unsafe { PROCESSES.get_mut() }.get_mut(caller.entity) {
                if let Some(old_brk) = process.change_program_brk(size as isize) {
                    old_brk as isize
                } else {
                    -1
                }
            } else {
                -1
            }
        }
    }

    /// Scheduling 系统调用实现
    impl Scheduling for SyscallContext {
        #[inline]
        fn sched_yield(&self, _caller: Caller) -> isize {
            0
        }
    }

    /// Clock 系统调用实现
    ///
    /// 与前章不同：需要通过 translate() 将用户传入的 TimeSpec 指针
    /// 翻译为内核可访问的物理地址，然后写入时间数据。
    impl Clock for SyscallContext {
        #[inline]
        fn clock_gettime(&self, caller: Caller, clock_id: ClockId, tp: usize) -> isize {
            // 检查用户地址是否可写
            const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
            match clock_id {
                ClockId::CLOCK_MONOTONIC => {
                    if let Some(mut ptr) = unsafe { PROCESSES.get_mut() }
                        .get_mut(caller.entity)
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

    /// Trace 系统调用实现（练习题需要完成的部分）
    ///
    /// 引入虚存机制后，原来的 trace 实现无效了，需要：
    /// - 读取时检查用户地址是否可见且可读
    /// - 写入时检查用户地址是否可见且可写
    /// - 使用 translate() 方法进行地址翻译和权限检查
    impl Trace for SyscallContext {
        #[inline]
        fn trace(
            &self,
            caller: Caller,
            trace_request: usize,
            id: usize,
            data: usize,
        ) -> isize {
            const READABLE: VmFlags<Sv39> = build_flags("RV");
            const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
            let Some(proc) = unsafe { PROCESSES.get_mut() }.get_mut(caller.entity) else {
                return -1;
            };
            match trace_request {
                0 => {
                    if !sv39_user_range_ptr(id) {
                        return -1;
                    }
                    if let Some(ptr) = proc
                        .address_space
                        .translate::<u8>(VAddr::new(id), READABLE)
                    {
                        unsafe { *ptr.as_ptr() as isize }
                    } else {
                        -1
                    }
                }
                1 => {
                    if !sv39_user_range_ptr(id) {
                        return -1;
                    }
                    if let Some(mut ptr) = proc
                        .address_space
                        .translate::<u8>(VAddr::new(id), WRITABLE)
                    {
                        unsafe {
                            *ptr.as_mut() = data as u8;
                        }
                        0
                    } else {
                        -1
                    }
                }
                2 => {
                    if id >= process::SYSCALL_COUNT_LEN {
                        return -1;
                    }
                    process::with_active_syscall_counts(|c| c[id] as isize).unwrap_or(-1)
                }
                _ => -1,
            }
        }
    }

    /// Sv39 用户态指针：须落在低半canonical 区（bit38..63 为 0），否则 `VAddr::new` 掩位后
    /// 可能与合法映射冲突（如 `isize::MAX as usize`）。
    #[inline]
    fn sv39_user_range_ptr(addr: usize) -> bool {
        addr >> 38 == 0
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

    /// Memory 系统调用实现（练习题需要完成的部分）
    ///
    /// - `mmap`：将物理内存映射到用户虚拟地址空间
    /// - `munmap`：取消虚拟内存映射
    impl Memory for SyscallContext {
        fn mmap(
            &self,
            caller: Caller,
            addr: usize,
            len: usize,
            prot: i32,
            _flags: i32,
            _fd: i32,
            _offset: usize,
        ) -> isize {
            const PAGE: usize = 1 << Sv39::PAGE_BITS;
            if addr & (PAGE - 1) != 0 {
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
            let vpn_s = VAddr::new(addr).floor();
            let vpn_e = VAddr::new(end_v).ceil();
            if vpn_s >= vpn_e {
                return 0;
            }
            let Some(proc) = unsafe { PROCESSES.get_mut() }.get_mut(caller.entity) else {
                return -1;
            };
            const READABLE: VmFlags<Sv39> = build_flags("RV");
            let mut v = vpn_s;
            while v < vpn_e {
                if proc
                    .address_space
                    .translate::<u8>(v.base(), READABLE)
                    .is_some()
                {
                    return -1;
                }
                v = v + 1;
            }
            proc.address_space.map(vpn_s..vpn_e, &[], 0, vm_flags);
            0
        }

        fn munmap(&self, caller: Caller, addr: usize, len: usize) -> isize {
            const PAGE: usize = 1 << Sv39::PAGE_BITS;
            if addr & (PAGE - 1) != 0 {
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
            let vpn_s = VAddr::new(addr).floor();
            let vpn_e = VAddr::new(end_v).ceil();
            if vpn_s >= vpn_e {
                return 0;
            }
            let Some(proc) = unsafe { PROCESSES.get_mut() }.get_mut(caller.entity) else {
                return -1;
            };
            const READABLE: VmFlags<Sv39> = build_flags("RV");
            let mut v = vpn_s;
            while v < vpn_e {
                if proc
                    .address_space
                    .translate::<u8>(v.base(), READABLE)
                    .is_none()
                {
                    return -1;
                }
                v = v + 1;
            }
            proc.address_space.unmap(vpn_s..vpn_e);
            0
        }
    }
}

/// 非 RISC-V64 架构的占位模块。
///
/// 提供编译所需的符号和类型，使得 `cargo publish --dry-run` 在主机平台上能通过编译。
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    use tg_kernel_vm::page_table::{MmuMeta, VmFlags};

    /// Sv39 占位类型：在主机平台上模拟 Sv39 的参数
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

    /// 构建 VmFlags 占位
    pub const fn build_flags(_s: &str) -> VmFlags<Sv39> {
        unsafe { VmFlags::from_raw(0) }
    }

    /// 解析 VmFlags 占位
    pub fn parse_flags(_s: &str) -> Result<VmFlags<Sv39>, ()> {
        Ok(unsafe { VmFlags::from_raw(0) })
    }

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

//! # 第一章：应用程序与基本执行环境（扩展：ch1-tangram 七巧板图案）
//!
//! 本章实现了一个最简单的 RISC-V S 态裸机程序，展示操作系统的最小执行环境。
//! 扩展实验（ch1-tangram）：通过 VirtIO-GPU 在 Framebuffer 上绘制七巧板 "OS" 图案。
//!
//! ## 关键概念
//!
//! - `#![no_std]`：不使用 Rust 标准库，改用不依赖操作系统的核心库 `core`
//! - `#![no_main]`：不使用标准的 `main` 入口，自定义裸函数 `_start` 作为入口
//! - 裸函数（naked function）：不生成函数序言/尾声，可在无栈环境下执行
//! - SBI（Supervisor Binary Interface）：S 态软件向 M 态固件请求服务的标准接口
//! - VirtIO-GPU：QEMU 提供的虚拟 GPU 设备，通过 MMIO 和 virtqueue 通信

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

/// S 态主函数：绘制七巧板 "OS" 图案并关机；副核仅打印上线信息后 `wfi` 等待（实验 9：ch1 多核）。
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
        for c in b"[ch1-tangram] VirtIO-GPU framebuffer demo\n" {
            console_putchar(*c);
        }
    });

    // 尝试初始化 VirtIO-GPU 并绘制七巧板
    match virtio_gpu::init_and_draw() {
        Ok(()) => {
            with_console_lock(|| {
                for c in b"[ch1-tangram] OK: tangram drawn to framebuffer\n" {
                    console_putchar(*c);
                }
            });
        }
        Err(e) => {
            with_console_lock(|| {
                for c in b"[ch1-tangram] GPU error: " {
                    console_putchar(*c);
                }
                for c in e.as_bytes() {
                    console_putchar(*c);
                }
                console_putchar(b'\n');
            });
        }
    }

    shutdown(false)
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

/// VirtIO-GPU 裸机驱动模块。
///
/// 使用 `virtio-drivers` crate 提供的 `VirtIOGpu` 驱动。
/// 通过一个静态内存池实现 `Hal` trait（零堆分配），
/// 在裸机 S 态（无操作系统、无分页）下操作 QEMU virt 平台的 VirtIO-GPU。
///
/// QEMU virt 平台（无分页时）虚拟地址 == 物理地址，DMA 地址直接用虚拟地址即可。
#[cfg(target_arch = "riscv64")]
mod virtio_gpu {
    use core::sync::atomic::{AtomicUsize, Ordering};
    use virtio_drivers::{Hal, MmioTransport, PhysAddr, VirtAddr, VirtIOGpu, VirtIOHeader};

    // ────────────────────────────────────────────────────────────────
    // 全局堆分配器（bump allocator，供 virtio-drivers 的 alloc 使用）
    // virtio-drivers 在内部使用 alloc::vec 等，因此需要全局分配器。
    // ────────────────────────────────────────────────────────────────
    const HEAP_SIZE: usize = 0x80_0000; // 8 MiB

    #[repr(C, align(4096))]
    struct HeapSpace([u8; HEAP_SIZE]);
    static mut HEAP_SPACE: HeapSpace = HeapSpace([0u8; HEAP_SIZE]);
    static HEAP_NEXT: AtomicUsize = AtomicUsize::new(0);

    struct BumpAlloc;

    unsafe impl core::alloc::GlobalAlloc for BumpAlloc {
        unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
            let align = layout.align();
            let size = layout.size();
            let base = (&raw mut HEAP_SPACE) as usize;
            loop {
                let cur = HEAP_NEXT.load(Ordering::Relaxed);
                let aligned = (base + cur + align - 1) & !(align - 1);
                let end = aligned - base + size;
                if end > HEAP_SIZE {
                    return core::ptr::null_mut();
                }
                if HEAP_NEXT.compare_exchange(cur, end, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                    return aligned as *mut u8;
                }
            }
        }
        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {
            // bump allocator，不释放
        }
    }

    #[global_allocator]
    static ALLOCATOR: BumpAlloc = BumpAlloc;

    // ────────────────────────────────────────────────────────────────
    // 静态 DMA 内存池（供 virtio-drivers Hal 使用）
    // ────────────────────────────────────────────────────────────────
    const PAGE_SIZE: usize = 0x1000;
    const POOL_PAGES: usize = 1600; // 6.25 MiB for DMA (covers 1280x800x4 + overhead)

    #[repr(C, align(4096))]
    struct DmaPool([u8; PAGE_SIZE * POOL_PAGES]);

    static mut DMA_POOL: DmaPool = DmaPool([0u8; PAGE_SIZE * POOL_PAGES]);
    static DMA_NEXT: AtomicUsize = AtomicUsize::new(0);

    /// 静态内存池 HAL 实现（bump allocator，无 free）。
    pub struct StaticHal;

    impl Hal for StaticHal {
        fn dma_alloc(pages: usize) -> PhysAddr {
            let offset = DMA_NEXT.fetch_add(pages, Ordering::Relaxed);
            if offset + pages > POOL_PAGES {
                return 0; // OOM
            }
            unsafe { ((&raw mut DMA_POOL) as *mut u8).add(offset * PAGE_SIZE) as usize }
        }

        fn dma_dealloc(_paddr: PhysAddr, _pages: usize) -> i32 {
            0 // bump allocator 不释放
        }

        fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
            paddr // 无分页，物理 == 虚拟
        }

        fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
            vaddr // 无分页，物理 == 虚拟
        }
    }

    // ────────────────────────────────────────────────────────────────
    // VirtIO-GPU 初始化 + 绘制
    // ────────────────────────────────────────────────────────────────

    /// 扫描 QEMU virt 平台的 VirtIO MMIO slot，返回 GPU 设备的基地址。
    fn find_gpu() -> Option<usize> {
        for slot in (0..8usize).rev() {
            // QEMU 按逆序分配：最高 slot = 0x1000_8000 对应第一个 -device
            let base = 0x1000_1000 + slot * 0x1000;
            let magic = unsafe { core::ptr::read_volatile(base as *const u32) };
            if magic != 0x7472_6976 {
                continue;
            }
            let dev_id = unsafe { core::ptr::read_volatile((base + 0x8) as *const u32) };
            if dev_id == 16 {
                // VirtIO GPU device ID
                return Some(base);
            }
        }
        None
    }

    /// 初始化 VirtIO-GPU 并绘制七巧板 "OS" 图案。
    pub fn init_and_draw() -> Result<(), &'static str> {
        let base = find_gpu().ok_or("no VirtIO-GPU device found")?;

        let header = core::ptr::NonNull::new(base as *mut VirtIOHeader).unwrap();
        let transport = unsafe {
            MmioTransport::new(header).map_err(|_| "MmioTransport::new failed")?
        };

        let mut gpu = VirtIOGpu::<StaticHal, MmioTransport>::new(transport)
            .map_err(|_| "VirtIOGpu::new failed")?;

        let (w, h) = gpu.resolution().map_err(|_| "resolution failed")?;

        let fb = gpu
            .setup_framebuffer()
            .map_err(|_| "setup_framebuffer failed")?;

        draw_tangram(fb, w, h);

        gpu.flush().map_err(|_| "flush failed")?;

        Ok(())
    }

    // ────────────────────────────────────────────────────────────────
    // 七巧板绘制逻辑
    // ────────────────────────────────────────────────────────────────

    /// 颜色：B8G8R8A8（小端存储：buf[0]=B, buf[1]=G, buf[2]=R, buf[3]=A）
    const BLACK: [u8; 4] = [0x10, 0x10, 0x10, 0xFF]; // 深色背景
    const RED: [u8; 4] = [0x30, 0x40, 0xE0, 0xFF];   // 红色
    const ORANGE: [u8; 4] = [0x00, 0x80, 0xFF, 0xFF]; // 橙色
    const YELLOW: [u8; 4] = [0x00, 0xCC, 0xFF, 0xFF]; // 黄色
    const GREEN: [u8; 4] = [0x30, 0xC0, 0x50, 0xFF];  // 绿色
    const BLUE: [u8; 4] = [0xD0, 0x60, 0x20, 0xFF];   // 蓝色
    const CYAN: [u8; 4] = [0xCC, 0xAA, 0x20, 0xFF];   // 青色
    const PURPLE: [u8; 4] = [0xB0, 0x30, 0x90, 0xFF]; // 紫色

    /// 设置像素（BGRA 格式）
    fn set_pixel(buf: &mut [u8], w: u32, x: u32, y: u32, color: [u8; 4]) {
        if x >= w {
            return;
        }
        let offset = ((y * w + x) * 4) as usize;
        if offset + 3 < buf.len() {
            buf[offset] = color[0];
            buf[offset + 1] = color[1];
            buf[offset + 2] = color[2];
            buf[offset + 3] = color[3];
        }
    }

    /// 填充矩形区域
    fn fill_rect(buf: &mut [u8], w: u32, x0: u32, y0: u32, rw: u32, rh: u32, color: [u8; 4]) {
        for dy in 0..rh {
            for dx in 0..rw {
                set_pixel(buf, w, x0 + dx, y0 + dy, color);
            }
        }
    }

    /// 填充等腰直角三角形（垂直方向）
    /// `dir`=0：顶角在上（▽形）；`dir`=1：顶角在下（△形）
    fn fill_triangle_v(
        buf: &mut [u8],
        w: u32,
        x0: u32,
        y0: u32,
        size: u32,
        color: [u8; 4],
        dir: u8,
    ) {
        let cx = x0 + size / 2;
        for row in 0..size {
            let (start_x, end_x) = if dir == 0 {
                let s = cx.saturating_sub(row);
                (s, cx + row + 1)
            } else {
                let inv = size - 1 - row;
                let s = cx.saturating_sub(inv);
                (s, cx + inv + 1)
            };
            for px in start_x..end_x {
                set_pixel(buf, w, px, y0 + row, color);
            }
        }
    }

    /// 填充平行四边形（向右倾斜）
    fn fill_parallelogram(
        buf: &mut [u8],
        w: u32,
        x0: u32,
        y0: u32,
        pw: u32,
        ph: u32,
        color: [u8; 4],
    ) {
        for row in 0..ph {
            for col in 0..pw {
                set_pixel(buf, w, x0 + col + row, y0 + row, color);
            }
        }
    }

    /// 绘制七巧板 "OS" 图案。
    ///
    /// 图案分两部分：左侧 "O"（正方形七巧板），右侧 "S"（七巧板拼接）。
    /// 使用标准七巧板的 7 块：大三角 ×2、中三角 ×1、小三角 ×2、正方形 ×1、平行四边形 ×1。
    fn draw_tangram(buf: &mut [u8], scr_w: u32, scr_h: u32) {
        // 先填充深色背景
        for y in 0..scr_h {
            for x in 0..scr_w {
                set_pixel(buf, scr_w, x, y, BLACK);
            }
        }

        // 七巧板单元格大小
        let unit = (scr_h.min(scr_w) / 6).max(40);
        let total_w = unit * 7;
        let start_x = (scr_w.saturating_sub(total_w)) / 2;
        let start_y = (scr_h.saturating_sub(unit * 4)) / 2;

        // ────── 字母 "O" ──────
        let ox = start_x;
        let oy = start_y;
        let s = unit;

        fill_triangle_v(buf, scr_w, ox, oy, s, RED, 0);
        fill_triangle_v(buf, scr_w, ox + s, oy, s, ORANGE, 0);
        fill_rect(buf, scr_w, ox, oy + s / 2, s / 2, s / 2, YELLOW);
        fill_parallelogram(buf, scr_w, ox + s / 2, oy + s / 2, s / 2, s / 4, GREEN);
        fill_triangle_v(buf, scr_w, ox + s / 4, oy + s, s, BLUE, 1);
        fill_triangle_v(buf, scr_w, ox, oy + s + s / 2, s / 2, CYAN, 1);
        fill_triangle_v(buf, scr_w, ox + s + s / 4, oy + s + s / 2, s / 2, PURPLE, 1);

        // ────── 字母 "S" ──────
        let sx = start_x + unit * 4;
        let sy = start_y;

        fill_triangle_v(buf, scr_w, sx + s / 2, sy, s, ORANGE, 0);
        fill_triangle_v(buf, scr_w, sx, sy, s / 2, RED, 0);
        fill_rect(buf, scr_w, sx, sy + s / 2, s, s / 4, CYAN);
        fill_triangle_v(buf, scr_w, sx, sy + s, s, BLUE, 1);
        fill_rect(buf, scr_w, sx, sy + s + s / 2, s, s / 4, GREEN);
        fill_triangle_v(buf, scr_w, sx + s / 2, sy + s + s / 2, s / 2, PURPLE, 1);
        fill_parallelogram(buf, scr_w, sx, sy + s + s / 2 + s / 4, s, s / 4, YELLOW);
    }
}

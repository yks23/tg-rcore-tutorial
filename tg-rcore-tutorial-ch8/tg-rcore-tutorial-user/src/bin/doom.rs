//! ch8-doom：用户态 DOOM 风格 Raycasting 软件渲染演示
//!
//! 完全使用整数定点数运算（避免 no_std 下 f32::sin/cos 不可用），
//! 通过 syscall 622/623 操作 VirtIO-GPU 帧缓冲。

#![no_std]
#![no_main]

extern crate alloc;
extern crate user_lib;

use alloc::vec::Vec;
use user_lib::{draw_framebuffer, get_fb_info, get_time, sched_yield};

// ─── 地图 ─────────────────────────────────────────────────────────────────────
const MAP_W: usize = 16;
const MAP_H: usize = 16;
#[rustfmt::skip]
const MAP: [u8; MAP_W * MAP_H] = [
    1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,
    1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,
    1,0,1,1,0,0,0,1,1,0,0,0,1,0,0,1,
    1,0,1,0,0,0,0,0,1,0,0,0,1,0,0,1,
    1,0,0,0,0,1,0,0,0,0,0,0,0,0,0,1,
    1,0,0,0,0,1,0,0,0,0,1,1,1,0,0,1,
    1,0,0,0,0,0,0,0,0,0,1,0,1,0,0,1,
    1,0,1,0,0,0,0,0,0,0,0,0,0,0,0,1,
    1,0,1,0,0,0,1,1,1,0,0,0,0,0,0,1,
    1,0,0,0,0,0,1,0,1,0,1,1,0,0,0,1,
    1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,
    1,0,1,1,1,0,0,0,0,0,0,0,1,0,0,1,
    1,0,0,0,0,0,0,0,1,0,0,0,1,0,0,1,
    1,0,0,0,0,0,0,0,1,0,0,0,0,0,0,1,
    1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,
    1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,
];

fn map_at(x: i32, y: i32) -> bool {
    if x < 0 || y < 0 || x >= MAP_W as i32 || y >= MAP_H as i32 { return true; }
    MAP[y as usize * MAP_W + x as usize] != 0
}

// ─── 定点数（16.16 格式）──────────────────────────────────────────────────────
const FP: i32 = 1 << 16;

// ─── sin/cos 表（512 个步长，对应 0..2π）─────────────────────────────────────
// sin(k * 2π/512) * 65536，k=0..511
const SIN_TABLE_LEN: usize = 512;
const SIN_DATA: [i32; SIN_TABLE_LEN] = [
        0,804,1608,2410,3212,4011,4808,5602,6393,7179,7962,8739,9512,10278,11039,11793,
        12539,13279,14010,14732,15446,16151,16846,17530,18204,18868,19519,20159,20787,21403,22005,22594,
        23170,23731,24279,24811,25329,25832,26319,26790,27245,27683,28105,28510,28898,29268,29621,29956,
        30273,30571,30852,31113,31356,31580,31785,31971,32137,32285,32412,32521,32609,32678,32728,32757,
        32768,32757,32728,32678,32609,32521,32412,32285,32137,31971,31785,31580,31356,31113,30852,30571,
        30273,29956,29621,29268,28898,28510,28105,27683,27245,26790,26319,25832,25329,24811,24279,23731,
        23170,22594,22005,21403,20787,20159,19519,18868,18204,17530,16846,16151,15446,14732,14010,13279,
        12539,11793,11039,10278,9512,8739,7962,7179,6393,5602,4808,4011,3212,2410,1608,804,
        0,-804,-1608,-2410,-3212,-4011,-4808,-5602,-6393,-7179,-7962,-8739,-9512,-10278,-11039,-11793,
        -12539,-13279,-14010,-14732,-15446,-16151,-16846,-17530,-18204,-18868,-19519,-20159,-20787,-21403,-22005,-22594,
        -23170,-23731,-24279,-24811,-25329,-25832,-26319,-26790,-27245,-27683,-28105,-28510,-28898,-29268,-29621,-29956,
        -30273,-30571,-30852,-31113,-31356,-31580,-31785,-31971,-32137,-32285,-32412,-32521,-32609,-32678,-32728,-32757,
        -32768,-32757,-32728,-32678,-32609,-32521,-32412,-32285,-32137,-31971,-31785,-31580,-31356,-31113,-30852,-30571,
        -30273,-29956,-29621,-29268,-28898,-28510,-28105,-27683,-27245,-26790,-26319,-25832,-25329,-24811,-24279,-23731,
        -23170,-22594,-22005,-21403,-20787,-20159,-19519,-18868,-18204,-17530,-16846,-16151,-15446,-14732,-14010,-13279,
        -12539,-11793,-11039,-10278,-9512,-8739,-7962,-7179,-6393,-5602,-4808,-4011,-3212,-2410,-1608,-804,
        0,804,1608,2410,3212,4011,4808,5602,6393,7179,7962,8739,9512,10278,11039,11793,
        12539,13279,14010,14732,15446,16151,16846,17530,18204,18868,19519,20159,20787,21403,22005,22594,
        23170,23731,24279,24811,25329,25832,26319,26790,27245,27683,28105,28510,28898,29268,29621,29956,
        30273,30571,30852,31113,31356,31580,31785,31971,32137,32285,32412,32521,32609,32678,32728,32757,
        32768,32757,32728,32678,32609,32521,32412,32285,32137,31971,31785,31580,31356,31113,30852,30571,
        30273,29956,29621,29268,28898,28510,28105,27683,27245,26790,26319,25832,25329,24811,24279,23731,
        23170,22594,22005,21403,20787,20159,19519,18868,18204,17530,16846,16151,15446,14732,14010,13279,
        12539,11793,11039,10278,9512,8739,7962,7179,6393,5602,4808,4011,3212,2410,1608,804,
        0,-804,-1608,-2410,-3212,-4011,-4808,-5602,-6393,-7179,-7962,-8739,-9512,-10278,-11039,-11793,
        -12539,-13279,-14010,-14732,-15446,-16151,-16846,-17530,-18204,-18868,-19519,-20159,-20787,-21403,-22005,-22594,
        -23170,-23731,-24279,-24811,-25329,-25832,-26319,-26790,-27245,-27683,-28105,-28510,-28898,-29268,-29621,-29956,
        -30273,-30571,-30852,-31113,-31356,-31580,-31785,-31971,-32137,-32285,-32412,-32521,-32609,-32678,-32728,-32757,
        -32768,-32757,-32728,-32678,-32609,-32521,-32412,-32285,-32137,-31971,-31785,-31580,-31356,-31113,-30852,-30571,
        -30273,-29956,-29621,-29268,-28898,-28510,-28105,-27683,-27245,-26790,-26319,-25832,-25329,-24811,-24279,-23731,
        -23170,-22594,-22005,-21403,-20787,-20159,-19519,-18868,-18204,-17530,-16846,-16151,-15446,-14732,-14010,-13279,
        -12539,-11793,-11039,-10278,-9512,-8739,-7962,-7179,-6393,-5602,-4808,-4011,-3212,-2410,-1608,-804,
];

fn tbl_sin(idx: i32) -> i32 {
    SIN_DATA[((idx % SIN_TABLE_LEN as i32 + SIN_TABLE_LEN as i32) % SIN_TABLE_LEN as i32) as usize]
}
fn tbl_cos(idx: i32) -> i32 {
    tbl_sin(idx + SIN_TABLE_LEN as i32 / 4)
}

// ─── 颜色（BGRA）──────────────────────────────────────────────────────────────
const CEILING: u32 = 0xFF383838;
const FLOOR: u32 = 0xFF504030;
const WALL_NS: u32 = 0xFF2060C0;
const WALL_EW: u32 = 0xFF1040A0;

// ─── 帧缓冲 ───────────────────────────────────────────────────────────────────
struct Fb {
    buf: Vec<u32>,
    w: u32,
    h: u32,
}

impl Fb {
    fn new(w: u32, h: u32) -> Self {
        Fb { buf: alloc::vec![0u32; (w * h) as usize], w, h }
    }

    #[inline]
    fn set(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && (x as u32) < self.w && (y as u32) < self.h {
            self.buf[(y as u32 * self.w + x as u32) as usize] = color;
        }
    }

    fn fill_col(&mut self, x: i32, y0: i32, y1: i32, color: u32) {
        let y0 = y0.max(0);
        let y1 = y1.min(self.h as i32 - 1);
        for y in y0..=y1 { self.set(x, y, color); }
    }

    fn clear_ceiling_floor(&mut self) {
        let h = self.h as i32;
        let w = self.w as i32;
        let mid = h / 2;
        for y in 0..h {
            let c = if y < mid { CEILING } else { FLOOR };
            for x in 0..w { self.set(x, y, c); }
        }
    }

    fn flush(&self) -> isize {
        let bytes = unsafe {
            core::slice::from_raw_parts(self.buf.as_ptr() as *const u8, self.buf.len() * 4)
        };
        draw_framebuffer(bytes, self.w, self.h)
    }
}

// ─── 暗化颜色（模拟远处阴影）──────────────────────────────────────────────────
fn darken(color: u32, dist_fp: i32, max_dist_fp: i32) -> u32 {
    let factor = ((max_dist_fp - dist_fp).max(0) * 256 / max_dist_fp.max(1)).min(255) as u32;
    let b = (color & 0xFF) * factor / 255;
    let g = ((color >> 8) & 0xFF) * factor / 255;
    let r = ((color >> 16) & 0xFF) * factor / 255;
    0xFF000000 | (r << 16) | (g << 8) | b
}

// ─── 主程序 ───────────────────────────────────────────────────────────────────
#[unsafe(no_mangle)]
fn main() -> i32 {
    let mut info = [0u32; 2];
    if get_fb_info(&mut info) < 0 { return -1; }
    let (sw, sh) = (info[0], info[1]);
    if sw == 0 || sh == 0 { return -1; }

    let mut fb = Fb::new(sw, sh);

    // 玩家位置（定点数 16.16）
    let mut px: i32 = 2 * FP + FP / 2;
    let mut py: i32 = 2 * FP + FP / 2;
    // 角度索引（0..512 对应 0..2π）
    let mut ang: i32 = 0;

    const STEP_FP: i32 = FP / 30; // 每帧移动
    const TURN: i32 = 3;           // 每帧转角（索引步长）
    const MAX_STEPS: i32 = 20;     // 最大 DDA 步数
    const MAX_DIST_FP: i32 = 14 * FP;

    // FOV: 60°对应 512 * 60/360 ≈ 85 个 table 单位
    let fov_half: i32 = SIN_TABLE_LEN as i32 * 60 / 360 / 2; // ~42

    let mut frame: u32 = 0;
    let mut last_t = get_time();
    const FRAME_MS: isize = 1000 / 30;

    loop {
        loop {
            if get_time() - last_t >= FRAME_MS { last_t = get_time(); break; }
            sched_yield();
        }
        frame += 1;

        // 自动移动：直行 + 定时转弯
        if frame % 80 < 60 {
            // 直行
            let nx = px + (tbl_cos(ang) * (STEP_FP >> 8) >> 8);
            let ny = py + (tbl_sin(ang) * (STEP_FP >> 8) >> 8);
            if !map_at(nx >> 16, py >> 16) { px = nx; }
            if !map_at(px >> 16, ny >> 16) { py = ny; }
        } else {
            ang = (ang + TURN) % SIN_TABLE_LEN as i32;
        }

        // ── 渲染 ──
        fb.clear_ceiling_floor();
        let sw_i = sw as i32;
        let sh_i = sh as i32;
        let half_sh = sh_i / 2;

        for col in 0..sw_i {
            // 列对应的射线角度
            let col_ang = ang + (col - sw_i / 2) * fov_half / (sw_i / 2);
            let ray_cos = tbl_cos(col_ang);
            let ray_sin = tbl_sin(col_ang);

            // DDA 初始化（整数格）
            let map_x0 = px >> 16;
            let map_y0 = py >> 16;
            let frac_x = px & 0xFFFF; // 小数部分 0..65535
            let frac_y = py & 0xFFFF;

            // 每格步长（用 FP*FP/cos，FP*FP/sin 近似）
            let dx_per_cell = if ray_cos != 0 { FP as i64 * FP as i64 / ray_cos.unsigned_abs() as i64 } else { i32::MAX as i64 };
            let dy_per_cell = if ray_sin != 0 { FP as i64 * FP as i64 / ray_sin.unsigned_abs() as i64 } else { i32::MAX as i64 };
            let dx_per_cell = dx_per_cell.min(i32::MAX as i64) as i32;
            let dy_per_cell = dy_per_cell.min(i32::MAX as i64) as i32;

            let (step_mx, mut sdx) = if ray_cos >= 0 {
                (1i32, (FP - frac_x) as i64 * dx_per_cell as i64 / FP as i64)
            } else {
                (-1i32, frac_x as i64 * dx_per_cell as i64 / FP as i64)
            };
            let (step_my, mut sdy) = if ray_sin >= 0 {
                (1i32, (FP - frac_y) as i64 * dy_per_cell as i64 / FP as i64)
            } else {
                (-1i32, frac_y as i64 * dy_per_cell as i64 / FP as i64)
            };
            let mut sdx = sdx as i32;
            let mut sdy = sdy as i32;

            let mut mx = map_x0;
            let mut my = map_y0;
            let mut hit = false;
            let mut side = 0u8;
            let mut dist_fp = 0i32;

            for _ in 0..MAX_STEPS {
                if sdx < sdy {
                    dist_fp = sdx;
                    sdx += dx_per_cell;
                    mx += step_mx;
                    side = 0;
                } else {
                    dist_fp = sdy;
                    sdy += dy_per_cell;
                    my += step_my;
                    side = 1;
                }
                if map_at(mx, my) { hit = true; break; }
                if dist_fp > MAX_DIST_FP { break; }
            }

            if !hit { continue; }

            // 去除鱼眼：乘以 cos(列偏差角)
            let col_offset_ang = col_ang - ang;
            let cos_offset = tbl_cos(col_offset_ang).max(1);
            let perp_dist = (dist_fp as i64 * cos_offset as i64 / FP as i64).max(FP as i64 / 4) as i32;

            // 投影高度
            let wall_h = (sh_i as i64 * FP as i64 / perp_dist as i64).min(sh_i as i64) as i32;
            let top = (half_sh - wall_h / 2).max(0);
            let bot = (half_sh + wall_h / 2).min(sh_i - 1);

            let base = if side == 0 { WALL_NS } else { WALL_EW };
            let color = darken(base, perp_dist, MAX_DIST_FP);
            fb.fill_col(col, top, bot, color);
        }

        // 小地图（左上角）
        for my in 0..MAP_H as i32 {
            for mx in 0..MAP_W as i32 {
                let c = if map_at(mx, my) { 0xFF888888 } else { 0xFF222222 };
                fb.set(mx * 3, my * 3, c);
                fb.set(mx * 3 + 1, my * 3, c);
                fb.set(mx * 3, my * 3 + 1, c);
                fb.set(mx * 3 + 1, my * 3 + 1, c);
            }
        }
        let ppx = (px >> 16) * 3 + 1;
        let ppy = (py >> 16) * 3 + 1;
        fb.set(ppx, ppy, 0xFFFFFF00); // 黄色玩家点

        fb.flush();

        if frame >= 300 { break; }
    }
    0
}

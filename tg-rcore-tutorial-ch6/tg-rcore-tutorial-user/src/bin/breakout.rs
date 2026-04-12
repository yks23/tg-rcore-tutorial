//! ch6-breakout：用户态打砖块游戏
//!
//! 通过自定义 syscall 622/623 操作 VirtIO-GPU 帧缓冲。
//! 游戏逻辑：挡板反弹小球，击碎砖块；碰到底部游戏结束。
//! 颜色格式：BGRA（蓝、绿、红、透明度），与 VirtIO-GPU Format::B8G8R8A8UNORM 一致。

#![no_std]
#![no_main]

extern crate alloc;
extern crate user_lib;

use user_lib::{draw_framebuffer, get_fb_info, get_time, sched_yield};

// ─── 颜色常量（BGRA 格式）─────────────────────────────────────────────────────
const BG: u32 = 0xFF1A1A2E;      // 深蓝背景
const PADDLE: u32 = 0xFF4CC9F0;  // 亮蓝挡板
const BALL: u32 = 0xFFFFD60A;    // 黄色小球
const BRICK_COLORS: [u32; 5] = [
    0xFFE63946, // 红
    0xFFFF9F1C, // 橙
    0xFFFFBF69, // 黄橙
    0xFF2EC4B6, // 青
    0xFF3A86FF, // 蓝
];
const TEXT_COLOR: u32 = 0xFFFFFFFF; // 白色文字

// ─── 游戏参数 ─────────────────────────────────────────────────────────────────
const BRICK_ROWS: usize = 5;
const BRICK_COLS: usize = 10;
const BRICK_H: i32 = 24;
const BRICK_PAD: i32 = 4;
const PADDLE_H: i32 = 12;
const BALL_R: i32 = 7;
const TARGET_FPS: i64 = 30;
const FRAME_MS: i64 = 1000 / TARGET_FPS;

// ─── 帧缓冲（静态，避免大块堆分配）──────────────────────────────────────────
// 最大分辨率 1280×800，BGRA 格式
const FB_MAX_W: u32 = 1280;
const FB_MAX_H: u32 = 800;
const FB_MAX_PIXELS: usize = (FB_MAX_W * FB_MAX_H) as usize;

#[repr(C, align(4096))]
struct StaticFb([u32; FB_MAX_PIXELS]);
static mut FB_DATA: StaticFb = StaticFb([0u32; FB_MAX_PIXELS]);

struct Fb {
    w: u32,
    h: u32,
}

impl Fb {
    fn new(w: u32, h: u32) -> Self {
        Fb { w: w.min(FB_MAX_W), h: h.min(FB_MAX_H) }
    }

    #[inline]
    fn set(&mut self, x: i32, y: i32, color: u32) {
        if x >= 0 && y >= 0 && (x as u32) < self.w && (y as u32) < self.h {
            let ptr = (&raw mut FB_DATA) as *mut u32;
            unsafe { ptr.add((y as u32 * self.w + x as u32) as usize).write(color); }
        }
    }

    fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        for dy in 0..h {
            for dx in 0..w {
                self.set(x + dx, y + dy, color);
            }
        }
    }

    fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, color: u32) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.set(cx + dx, cy + dy, color);
                }
            }
        }
    }

    fn clear(&mut self) {
        let n = (self.w * self.h) as usize;
        let ptr = (&raw mut FB_DATA) as *mut u32;
        unsafe {
            for i in 0..n { ptr.add(i).write(BG); }
        }
    }

    fn flush(&self) -> isize {
        let ptr = (&raw const FB_DATA) as *const u8;
        let bytes = unsafe {
            core::slice::from_raw_parts(ptr, (self.w * self.h * 4) as usize)
        };
        draw_framebuffer(bytes, self.w, self.h)
    }
}

// ─── 游戏状态 ─────────────────────────────────────────────────────────────────

struct Bricks {
    alive: [[bool; BRICK_COLS]; BRICK_ROWS],
    count: usize,
}

impl Bricks {
    fn new() -> Self {
        Bricks {
            alive: [[true; BRICK_COLS]; BRICK_ROWS],
            count: BRICK_ROWS * BRICK_COLS,
        }
    }

    fn brick_rect(w: u32, row: usize, col: usize) -> (i32, i32, i32, i32) {
        let margin_x = 20i32;
        let margin_y = 40i32;
        let brick_w = (w as i32 - margin_x * 2 - BRICK_PAD * (BRICK_COLS as i32 - 1))
            / BRICK_COLS as i32;
        let bx = margin_x + col as i32 * (brick_w + BRICK_PAD);
        let by = margin_y + row as i32 * (BRICK_H + BRICK_PAD);
        (bx, by, brick_w, BRICK_H)
    }
}

// ─── 主程序 ───────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
fn main() -> i32 {
    let mut info = [0u32; 2];
    if get_fb_info(&mut info) < 0 {
        return -1;
    }
    let (w, h) = (info[0], info[1]);
    if w == 0 || h == 0 {
        return -1;
    }

    let mut fb = Fb::new(w, h);

    let paddle_w = (w as i32 / 5).max(60);
    let paddle_y = h as i32 - 40;
    let mut paddle_x = (w as i32 - paddle_w) / 2;

    let mut ball_x = w as i32 / 2;
    let mut ball_y = h as i32 / 2;
    let mut ball_dx = 4i32;
    let mut ball_dy = -4i32;

    let mut bricks = Bricks::new();
    let mut score = 0u32;
    let mut lives = 3u32;
    let mut frame = 0u64;
    let mut last_t = get_time();

    'game: loop {
        // ── 帧率限制 ──
        loop {
            let now = get_time();
            if now - last_t >= FRAME_MS as isize {
                last_t = now;
                break;
            }
            sched_yield();
        }
        frame += 1;

        // ── AI 挡板追球（自动演示）──
        let mid_paddle = paddle_x + paddle_w / 2;
        if ball_x > mid_paddle + 2 {
            paddle_x = (paddle_x + 5).min(w as i32 - paddle_w);
        } else if ball_x < mid_paddle - 2 {
            paddle_x = (paddle_x - 5).max(0);
        }

        // ── 物理更新 ──
        ball_x += ball_dx;
        ball_y += ball_dy;

        // 左右墙
        if ball_x - BALL_R <= 0 { ball_dx = ball_dx.abs(); }
        if ball_x + BALL_R >= w as i32 { ball_dx = -ball_dx.abs(); }
        // 上墙
        if ball_y - BALL_R <= 0 { ball_dy = ball_dy.abs(); }

        // 挡板碰撞
        if ball_y + BALL_R >= paddle_y
            && ball_x >= paddle_x
            && ball_x <= paddle_x + paddle_w
            && ball_dy > 0
        {
            ball_dy = -ball_dy;
            // 根据碰撞位置调整水平速度
            let offset = ball_x - (paddle_x + paddle_w / 2);
            ball_dx = offset / 10;
            if ball_dx == 0 { ball_dx = 1; }
        }

        // 底部掉球
        if ball_y + BALL_R > h as i32 {
            lives = lives.saturating_sub(1);
            if lives == 0 { break 'game; }
            ball_x = w as i32 / 2;
            ball_y = h as i32 / 2;
            ball_dx = if frame % 2 == 0 { 4 } else { -4 };
            ball_dy = -4;
        }

        // 砖块碰撞
        for row in 0..BRICK_ROWS {
            for col in 0..BRICK_COLS {
                if !bricks.alive[row][col] { continue; }
                let (bx, by, bw, bh) = Bricks::brick_rect(w, row, col);
                if ball_x + BALL_R >= bx
                    && ball_x - BALL_R <= bx + bw
                    && ball_y + BALL_R >= by
                    && ball_y - BALL_R <= by + bh
                {
                    bricks.alive[row][col] = false;
                    bricks.count -= 1;
                    score += 10;
                    ball_dy = -ball_dy;
                }
            }
        }

        if bricks.count == 0 {
            // 所有砖块消灭，重置
            bricks = Bricks::new();
            score += 100;
        }

        // ── 渲染 ──
        fb.clear();

        // 砖块
        for row in 0..BRICK_ROWS {
            for col in 0..BRICK_COLS {
                if !bricks.alive[row][col] { continue; }
                let (bx, by, bw, bh) = Bricks::brick_rect(w, row, col);
                let color = BRICK_COLORS[row % BRICK_COLORS.len()];
                fb.fill_rect(bx, by, bw, bh, color);
            }
        }

        // 挡板
        fb.fill_rect(paddle_x, paddle_y, paddle_w, PADDLE_H, PADDLE);

        // 小球
        fb.fill_circle(ball_x, ball_y, BALL_R, BALL);

        // 简单数字分数显示（右上角小点阵）
        draw_score(&mut fb, score, w as i32 - 80, 8);
        draw_lives(&mut fb, lives, 8, 8);

        fb.flush();

        // 运行 300 帧后自动退出（show.sh demo 模式截图用）
        if frame >= 300 {
            break;
        }
    }

    // 游戏结束画面
    fb.clear();
    fb.fill_rect(w as i32 / 2 - 60, h as i32 / 2 - 20, 120, 40, PADDLE);
    fb.flush();

    0
}

/// 用小矩形点阵显示分数（右上角）
fn draw_score(fb: &mut Fb, score: u32, x: i32, y: i32) {
    let (digits, len) = score_digits(score);
    let mut cx = x;
    for i in 0..len {
        draw_digit(fb, cx, y, digits[i], TEXT_COLOR);
        cx += 10;
    }
}

/// 显示剩余生命数
fn draw_lives(fb: &mut Fb, lives: u32, x: i32, y: i32) {
    for i in 0..lives as i32 {
        fb.fill_circle(x + i * 14, y + 5, 5, BALL);
    }
}

fn score_digits(mut n: u32) -> ([u8; 10], usize) {
    let mut digits = [0u8; 10];
    if n == 0 { return (digits, 1); }
    let mut len = 0;
    let mut tmp = [0u8; 10];
    while n > 0 { tmp[len] = (n % 10) as u8; len += 1; n /= 10; }
    for i in 0..len { digits[i] = tmp[len - 1 - i]; }
    (digits, len)
}

/// 5×7 点阵数字（超简化版）
fn draw_digit(fb: &mut Fb, x: i32, y: i32, d: u8, color: u32) {
    // 用 4×6 像素点阵，每个数字编码为 6 行 × 4 位
    const DIGITS: [[u8; 6]; 10] = [
        [0b1110, 0b1010, 0b1010, 0b1010, 0b1010, 0b1110], // 0
        [0b0110, 0b0010, 0b0010, 0b0010, 0b0010, 0b0010], // 1
        [0b1110, 0b0010, 0b0010, 0b1110, 0b1000, 0b1110], // 2
        [0b1110, 0b0010, 0b0010, 0b0110, 0b0010, 0b1110], // 3
        [0b1010, 0b1010, 0b1110, 0b0010, 0b0010, 0b0010], // 4
        [0b1110, 0b1000, 0b1110, 0b0010, 0b0010, 0b1110], // 5
        [0b1110, 0b1000, 0b1110, 0b1010, 0b1010, 0b1110], // 6
        [0b1110, 0b0010, 0b0010, 0b0010, 0b0010, 0b0010], // 7
        [0b1110, 0b1010, 0b1110, 0b1010, 0b1010, 0b1110], // 8
        [0b1110, 0b1010, 0b1110, 0b0010, 0b0010, 0b1110], // 9
    ];
    if d > 9 { return; }
    let rows = DIGITS[d as usize];
    for (ry, row) in rows.iter().enumerate() {
        for bit in 0..4usize {
            if row & (1 << (3 - bit)) != 0 {
                fb.set(x + bit as i32, y + ry as i32, color);
            }
        }
    }
}

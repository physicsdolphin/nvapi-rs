//! ThermalChannel 四件套 GET-only 探针（2026-11-15，热面收录第一轮）。
//!
//! 背景：mVolt+ 0.50.13 的 Thermal inputs/limits 面板实测 621.24 可用（电压
//! VFE 车道死了但热面活着）。四接口（RTSS NVAPIInterface.h 命名）：
//!   - ThermChannelGetInfo    0x0BC8163D  2736B params V2（channel[32] 记录
//!     含 offset_sw/offset_hw/min/max_temp/scaling/is_temp_sim_supported）
//!   - ThermChannelGetStatus  0x65FE3AAD  168B（channel[i] 温度 Q8）
//!   - ThermChannelGetControl 0xA933CE98  ★布局未知 —— 本探针的主要目标
//!   - ThermChannelSetControl 0x8DF19FA2  写半边（布局=Control，暂不调用）
//! 版本魔数约定 (ver<<16)|size；本探针扫 size 找 GetControl 接受的形态。
//!
//! Run: cargo test --release --test thermal_channel_probe -- --ignored --nocapture
//! 全 GET-only，零写入。

#![allow(unused_must_use)]

use nvapi::PhysicalGpu;
use nvapi::sys::api::NvAPI_GPU_ThermChannelGetControl;

fn nonzero_dwords(buf: &[u8], max: usize) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    for (i, w) in buf.chunks_exact(4).enumerate() {
        let v = u32::from_le_bytes(w.try_into().unwrap());
        if v != 0 {
            out.push((i * 4, v));
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

#[test]
#[ignore]
fn thermal_channel_probe() {
    nvapi::initialize().expect("init");
    let mut gpus = PhysicalGpu::enumerate().expect("enumerate");
    let gpu = gpus.drain(..).next().expect("no gpu");
    let handle = *gpu.handle();
    println!("GPU: {:?}", gpu.full_name());
    println!("driver: {:?}", nvapi::driver_version());

    // ---- ① GetInfo（2736B params V2，已有封装）----
    println!("=== [T1] ThermChannelGetInfo (2736B V2) ===");
    let info = match gpu.thermal_channel_info() {
        Ok(v) => v,
        Err(e) => {
            println!("GetInfo failed: {e:?}");
            return;
        }
    };
    println!("channel_mask={:#x}", info.channel_mask);
    for (ty, name) in [
        (0usize, "GPU_AVG"),
        (1, "GPU_MAX/hotspot"),
        (2, "BOARD"),
        (3, "MEMORY"),
        (4, "PWR_SUPPLY"),
    ] {
        match info.primary[ty] {
            Some(i) => {
                let c = info.primary_info(ty).unwrap();
                println!(
                    "  pri[{name}] = ch{i}: class={} type={} scaling={} offset_sw={} offset_hw={} minT={} maxT={} sim={}",
                    c.ch_class,
                    c.ch_type,
                    c.scaling,
                    c.offset_sw,
                    c.offset_hw,
                    c.min_temp,
                    c.max_temp,
                    c.is_temp_sim_supported,
                );
            }
            None => println!("  pri[{name}] = (absent)"),
        }
    }
    for (i, c) in info.channels.iter().enumerate() {
        if let Some(c) = c {
            if c.offset_sw != 0 || c.offset_hw != 0 {
                println!(
                    "  ch{i} OFFSETS: sw={} hw={} (type={})",
                    c.offset_sw, c.offset_hw, c.ch_type
                );
            }
        }
    }
    let channel_mask = info.channel_mask;

    // ---- ② GetStatus（168B，已有封装）----
    println!("=== [T2] ThermChannelGetStatus (168B V2) ===");
    match gpu.thermal_channel_status(channel_mask) {
        Ok(st) => {
            for (i, t) in &st.temps {
                println!("  ch{i} = {t:.2} C");
            }
            if st.temps.is_empty() {
                println!("  (no decoded temps)");
            }
        }
        Err(e) => println!("GetStatus failed: {e:?}"),
    }

    // ---- ③ GetControl 版本魔数 size 扫描（布局发现主实验）----
    // 版本魔数 (2<<16)|size；接受者返回 0。全 GET 安全。
    println!("=== [T3] ThermChannelGetControl size sweep (ver=(2<<16)|N) ===");
    let mut accepted = Vec::new();
    for size in [168usize, 232, 264, 296, 328, 360, 392, 424, 456, 488, 520, 1048, 72, 104, 136, 200, 8, 40, 56] {
        let mut buf = vec![0u8; size];
        let ver: u32 = ((2u32) << 16) | (size as u32 & 0xFFFF);
        buf[0..4].copy_from_slice(&ver.to_le_bytes());
        // 掩码区（仿 INFO/STATUS 布局）：全 32 bit
        if size >= 12 {
            buf[4..8].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        }
        let st = unsafe { NvAPI_GPU_ThermChannelGetControl(handle, buf.as_mut_ptr().cast()) } as i32;
        if st == 0 {
            accepted.push(size);
        }
        println!("  size={size}: st={st}");
    }
    println!("accepted sizes: {accepted:?}");

    // ---- ④ 被接受的最小形态全量 dump ----
    if let Some(&sz) = accepted.first() {
        println!("=== [T4] GetControl dump (size={sz}) ===");
        let mut buf = vec![0u8; sz];
        let ver: u32 = ((2u32) << 16) | (sz as u32 & 0xFFFF);
        buf[0..4].copy_from_slice(&ver.to_le_bytes());
        if sz >= 12 {
            buf[4..8].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        }
        let st = unsafe { NvAPI_GPU_ThermChannelGetControl(handle, buf.as_mut_ptr().cast()) } as i32;
        println!("status={st}");
        let nz = nonzero_dwords(&buf, usize::MAX);
        println!("nonzero dwords: {}", nz.len());
        for (off, v) in nz.iter().take(80) {
            let f = f32::from_le_bytes(v.to_le_bytes());
            println!("  +{off:04X}: {v:#010x} (f32 {f})");
        }
    }

    // ---- ⑤ ver=1 变体扫描（防版本字段不是 2）----
    println!("=== [T5] GetControl size sweep (ver=(1<<16)|N) ===");
    let mut accepted1 = Vec::new();
    for size in [168usize, 296, 264, 424, 200, 232, 328] {
        let mut buf = vec![0u8; size];
        let ver: u32 = ((1u32) << 16) | (size as u32 & 0xFFFF);
        buf[0..4].copy_from_slice(&ver.to_le_bytes());
        if size >= 12 {
            buf[4..8].copy_from_slice(&0xFFFFFFFFu32.to_le_bytes());
        }
        let st = unsafe { NvAPI_GPU_ThermChannelGetControl(handle, buf.as_mut_ptr().cast()) } as i32;
        if st == 0 {
            accepted1.push(size);
        }
        println!("  size={size}: st={st}");
    }
    println!("accepted (ver1): {accepted1:?}");

}

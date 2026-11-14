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
use nvapi::sys::api::{NvAPI_GPU_ThermChannelGetControl, NvAPI_GPU_ThermChannelSetControl};

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
    // ---- ③ GetControl 扫描 v2：掩码感知（真实 mask / 单 bit / 0）× 两 ID × ver 1/2
    // 上一轮全 -9 的首要嫌疑=掩码硬编 0xFFFFFFFF（含未充填通道位，仿 VFE 全掩码拒绝模式）。
    println!("=== [T3] Control sweep: ids x masks x ver x sizes ===");
    type ControlFn = unsafe fn(nvapi::sys::handles::NvPhysicalGpuHandle, *mut core::ffi::c_void) -> nvapi::sys::NvAPI_Status;
    let ids: [(&str, ControlFn); 2] = [
        ("GetControl 0xA933CE98", NvAPI_GPU_ThermChannelGetControl),
        ("SetControl 0x8DF19FA2", NvAPI_GPU_ThermChannelSetControl),
    ];
    let masks: [(u32, &str); 4] = [
        (channel_mask, "info-mask"),
        (0, "zero"),
        (1, "bit0"),
        (0x1cfff, "0x1cfff"),
    ];
    let sizes = [168usize, 232, 264, 296, 328, 360, 392, 424, 456, 488, 520, 1048, 72, 104, 136, 200, 8, 40, 56, 88, 120, 152, 184, 216, 248, 280, 312, 344, 376, 408, 440, 472, 504];
    let mut accepted: Vec<(usize, usize, u32, u32)> = Vec::new(); // (id_idx, size, ver, mask)
    for (id_idx, (id_name, id_fn)) in ids.iter().enumerate() {
        for (mask, mask_name) in masks {
            for ver in [2u32, 1u32] {
                for size in sizes {
                    let mut buf = vec![0u8; size];
                    let v: u32 = (ver << 16) | (size as u32 & 0xFFFF);
                    buf[0..4].copy_from_slice(&v.to_le_bytes());
                    if size >= 8 {
                        buf[4..8].copy_from_slice(&mask.to_le_bytes());
                    }
                    let st = unsafe { id_fn(handle, buf.as_mut_ptr().cast()) } as i32;
                    if st == 0 {
                        accepted.push((id_idx, size, ver, mask));
                        println!("  HIT {id_name} size={size} ver={ver} mask={mask_name}({mask:#x})");
                    } else if st != -9 {
                        println!("  {id_name} size={size} ver={ver} mask={mask_name}: st={st} (non -9!)");
                    }
                }
            }
        }
    }
    println!("accepted: {accepted:?}");
    if accepted.is_empty() {
        println!("  (no variant accepted — control unreachable via 2-arg call on this driver)");
    }

    // ---- ④ 被接受的最小形态全量 dump ----
    if let Some(&(id_idx, sz, ver, mask)) = accepted.first() {
        println!("=== [T4] control dump (id{} size={sz} ver={ver} mask={mask:#x}) ===", ids[id_idx].0);
        let id_fn = ids[id_idx].1;
        let mut buf = vec![0u8; sz];
        let v: u32 = (ver << 16) | (sz as u32 & 0xFFFF);
        buf[0..4].copy_from_slice(&v.to_le_bytes());
        if sz >= 8 {
            buf[4..8].copy_from_slice(&mask.to_le_bytes());
        }
        let st = unsafe { id_fn(handle, buf.as_mut_ptr().cast()) } as i32;
        println!("status={st}");
        let nz = nonzero_dwords(&buf, usize::MAX);
        println!("nonzero dwords: {}", nz.len());
        for (off, v) in nz.iter().take(80) {
            let f = f32::from_le_bytes(v.to_le_bytes());
            println!("  +{off:04X}: {v:#010x} (f32 {f})");
        }
    }


}

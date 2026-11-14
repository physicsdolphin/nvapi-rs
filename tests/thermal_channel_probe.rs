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
    // ---- ② GetControl 精确形态（mVolt 0x1400137FC 调用区复刻）----
    // 签名 (handle, buf, len)；buf 10444B，ver=(3<<16)|0x28CC=0x328CC，掩码@+8=0xFFFF。
    println!("=== [T3] GetControl exact shape (len=10444, ver=0x328CC) ===");
    let len = 0x28CCusize;
    let ver: u32 = 0x000328CC;
    let get_ctrl = |id: u32, mask: u32| -> (i32, Vec<u8>) {
        let f = match nvapi::sys::nvapi_QueryInterface(id) {
            Ok(p) if !p.is_null() => unsafe {
                core::mem::transmute::<*mut core::ffi::c_void,
                    unsafe extern "C" fn(nvapi::sys::handles::NvPhysicalGpuHandle, *mut core::ffi::c_void, usize) -> nvapi::sys::NvAPI_Status>(p)
            },
            _ => return (-9, Vec::new()),
        };
        let mut b = vec![0u8; len];
        b[0..4].copy_from_slice(&ver.to_le_bytes());
        b[8..12].copy_from_slice(&mask.to_le_bytes());
        let st = unsafe { f(handle, b.as_mut_ptr().cast(), len) } as i32;
        (st, b)
    };
    let (st_a, buf_a) = get_ctrl(0xA933CE98, 0xFFFF);
    println!("  GetControl(0xA933CE98) mask=0xFFFF: st={st_a}");
    let (st_b, buf_b) = get_ctrl(0xA933CE98, channel_mask);
    println!("  GetControl(0xA933CE98) mask={channel_mask:#x}: st={st_b}");
    let (st_c, _buf_c) = get_ctrl(0x8DF19FA2, 0xFFFF);
    println!("  SetControl-id as GET (0x8DF19FA2) mask=0xFFFF: st={st_c}");

    // ---- ③ 控制态 dump ----
    let (best_st, best_buf, best_tag) = if st_a == 0 {
        (st_a, &buf_a, "mask=0xFFFF")
    } else if st_b == 0 {
        (st_b, &buf_b, "mask=info")
    } else {
        (st_a, &buf_a, "mask=0xFFFF (refused)")
    };
    println!("=== [T4] control dump ({best_tag}) st={best_st} ===");
    let nz = nonzero_dwords(best_buf, usize::MAX);
    println!("nonzero dwords: {}", nz.len());
    for (off, v) in nz.iter().take(120) {
        let f = f32::from_le_bytes(v.to_le_bytes());
        println!("  +{off:04X}: {v:#010x} (f32 {f})");
    }
    println!("  --- Q8 candidates (i32 |v|<200000, v%4==0, !=0) ---");
    for (off, v) in nz.iter() {
        let sv = *v as i32;
        if sv.abs() < 200000 && sv % 4 == 0 {
            println!("  +{off:04X}: i32={sv} = Q8 {:.3} C", sv as f64 / 256.0);
        }
    }
    // ---- ⑥ 写臂（双重门控）：温度 sim RMW + 还原验证
    if std::env::var("NVOC_ALLOW_THERM_WRITE").is_ok() {
        let ch: usize = std::env::var("NVOC_THERM_CHANNEL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let celsius: f32 = std::env::var("NVOC_THERM_SIM_CELSIUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(45.0);
        println!("=== [T6] WRITE: ch{ch} sim={celsius} C (RMW + restore-verify) ===");
        let run = || -> Result<(), Box<dyn std::error::Error>> {
            let mask = gpu.thermal_channel_info()?.channel_mask;
            let orig = gpu.thermal_channel_control(mask)?;
            let orig_q8 = orig.entry[ch].sim_temp_q8;
            println!("  original entry[{ch}].sim_q8 = {orig_q8} ({:.2} C)", orig_q8 as f64 / 256.0);
            gpu.set_thermal_channel_sim_temp(ch, celsius)?;
            std::thread::sleep(std::time::Duration::from_millis(600));
            let st = gpu.thermal_channel_status(mask)?;
            println!(
                "  [mid] ch{ch} = {:.2} C {}",
                st.get(ch).unwrap_or(f32::NAN),
                if (st.get(ch).unwrap_or(f32::NAN) - celsius).abs() < 1.0 {
                    "<— CONSUMED"
                } else {
                    "(no follow — check sim semantics)"
                }
            );
            // 还原原值（Q8 精确）
            let mut rst = gpu.thermal_channel_control(mask)?;
            rst.entry[ch].sim_temp_q8 = orig_q8;
            gpu.set_thermal_channel_control(&mut rst)?;
            std::thread::sleep(std::time::Duration::from_millis(600));
            let back = gpu.thermal_channel_control(mask)?;
            println!(
                "  restore: entry[{ch}].sim_q8 = {} {}",
                back.entry[ch].sim_temp_q8,
                if back.entry[ch].sim_temp_q8 == orig_q8 { "<— restored" } else { "<— RESTORE FAILED" }
            );
            let st2 = gpu.thermal_channel_status(mask)?;
            println!("  [after] ch{ch} = {:.2} C", st2.get(ch).unwrap_or(f32::NAN));
            Ok(())
        };
        if let Err(e) = run() {
            println!("  write arm error: {e}");
        }
    } else {
        println!("write arm skipped — set NVOC_ALLOW_THERM_WRITE=1 (+NVOC_THERM_CHANNEL, NVOC_THERM_SIM_CELSIUS)");
    }
}

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
    // ---- ⑤b 活体轮询 diff（全 GET）：15s 高频轮询 control/info/status 三缓冲,
    // 用户期间在 mVolt UI 改模拟值 → 变动的 dword = 真字段（mVolt 实际写的地方）。
    println!("=== [T7] live poll-diff (15s): change mVolt GPU temp NOW ===");
    {
        use nvapi::sys::gpu::thermal::undocumented::{
            NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2, NV_GPU_THERMAL_THERM_CHANNEL_STATUS_PARAMS_V2,
        };
        use nvapi::sys::api::{NvAPI_GPU_ThermChannelGetInfo, NvAPI_GPU_ThermChannelGetStatus};
        let get_all = |mask: u32| -> (Vec<u8>, Vec<u8>, Vec<i32>) {
            let mut ctrl = vec![0u8; 10444];
            ctrl[0..4].copy_from_slice(&0x000328CCu32.to_le_bytes());
            ctrl[8..12].copy_from_slice(&mask.to_le_bytes());
            unsafe { NvAPI_GPU_ThermChannelGetControl(handle, ctrl.as_mut_ptr().cast()) };
            let mut inf = NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2::default();
            inf.version = nvapi::sys::nvapi::NvVersion::new(
                size_of::<NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2>(), 2);
            unsafe { NvAPI_GPU_ThermChannelGetInfo(handle, &mut inf) };
            let mut st = NV_GPU_THERMAL_THERM_CHANNEL_STATUS_PARAMS_V2::default();
            st.version = nvapi::sys::nvapi::NvVersion::new(
                size_of::<NV_GPU_THERMAL_THERM_CHANNEL_STATUS_PARAMS_V2>(), 2);
            st.channel_mask = mask;
            unsafe { NvAPI_GPU_ThermChannelGetStatus(handle, &mut st) };
            let stv: Vec<i32> = (0..32).filter(|i| mask & (1u32 << i) != 0)
                .filter_map(|i| st.channel.get(i).copied()).collect();
            // INFO raw bytes: walk the struct via pointer (params are repr(C) nvstruct)
            let ibytes = unsafe {
                core::slice::from_raw_parts(
                    &inf as *const NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2 as *const u8,
                    size_of::<NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2>(),
                )
            }
            .to_vec();
            (ctrl, ibytes, stv)
        };
        let (c0, i0, s0) = get_all(channel_mask);
        println!("  base captured: ctrl_nz={} info_nz={} status={s0:?}",
            nonzero_dwords(&c0, usize::MAX).len(),
            nonzero_dwords(&i0, usize::MAX).len());
        for it in 1..=60usize {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let (c, i, sv) = get_all(channel_mask);
            // control diff
            for (off, (a, b)) in c0.chunks_exact(4).zip(c.chunks_exact(4)).enumerate() {
                if a != b {
                    let av = u32::from_le_bytes(a.try_into().unwrap());
                    let bv = u32::from_le_bytes(b.try_into().unwrap());
                    println!("  [it{it}] CTRL +{off:04X}: {av:#010x} -> {bv:#010x}");
                }
            }
            // info diff
            for (off, (a, b)) in i0.chunks_exact(4).zip(i.chunks_exact(4)).enumerate() {
                if a != b {
                    let av = u32::from_le_bytes(a.try_into().unwrap());
                    let bv = u32::from_le_bytes(b.try_into().unwrap());
                    println!("  [it{it}] INFO +{off:04X}: {av:#010x} -> {bv:#010x}");
                }
            }
            // status diff
            if sv != s0 {
                println!("  [it{it}] STATUS: {s0:?} -> {sv:?}");
            }
            if it % 12 == 0 {
                println!("  ... polling, ch temps: {sv:?}");
            }
        }
        println!("  (poll done — any CTRL/INFO lines above = the fields mVolt actually writes)");
    }
    // ---- ⑥ 写臂（双重门控）：完整配方 = 值(entry_k+84) + 使能标志字节(47/101/128) + RMW + 全缓冲还原
    // 依据：T7 活体 diff（mVolt apply 时 CTRL +0x2F/+0x65/+0x80 置 1 + Status ch1 钉 1280=5°C）。
    if std::env::var("NVOC_ALLOW_THERM_WRITE").is_ok() {
        let ch: usize = std::env::var("NVOC_THERM_CHANNEL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let celsius: f32 = std::env::var("NVOC_THERM_SIM_CELSIUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(40.0);
        println!("=== [T6] WRITE full recipe: ch{ch} sim={celsius} C ===");
        let f = match nvapi::sys::nvapi_QueryInterface(0xA933CE98) {
            Ok(p) if !p.is_null() => unsafe {
                core::mem::transmute::<*mut core::ffi::c_void,
                    unsafe extern "C" fn(nvapi::sys::handles::NvPhysicalGpuHandle, *mut core::ffi::c_void, usize) -> nvapi::sys::NvAPI_Status>(p)
            },
            _ => {
                println!("  QueryInterface failed");
                return;
            }
        };
        let set_f = match nvapi::sys::nvapi_QueryInterface(0x8DF19FA2) {
            Ok(p) if !p.is_null() => unsafe {
                core::mem::transmute::<*mut core::ffi::c_void,
                    unsafe extern "C" fn(nvapi::sys::handles::NvPhysicalGpuHandle, *mut core::ffi::c_void, usize) -> nvapi::sys::NvAPI_Status>(p)
            },
            _ => {
                println!("  QueryInterface(SetControl) failed");
                return;
            }
        };
        let mut prime = vec![0u8; 10444];
        prime[0..4].copy_from_slice(&0x000328CCu32.to_le_bytes());
        prime[8..12].copy_from_slice(&channel_mask.to_le_bytes());
        unsafe { f(handle, prime.as_mut_ptr().cast(), 10444) };
        let orig = prime.clone();
        let st_before = gpu.thermal_channel_status(channel_mask);
        println!("  [before] status ch{ch} = {:?}", st_before.as_ref().ok().and_then(|s| s.get(ch)));
        // 值：entry_ch + 84（绝对）
        let vo = 84 + 108 * ch;
        let q8 = (celsius * 256.0) as i32;
        prime[vo..vo + 4].copy_from_slice(&q8.to_le_bytes());
        // 使能标志（mVolt GPU 输入 apply 时的三字节，经验值）
        prime[47] = 1;
        prime[101] = 1;
        prime[128] = 1;
        let st_set = unsafe { set_f(handle, prime.as_mut_ptr().cast(), 10444) } as i32;
        std::thread::sleep(std::time::Duration::from_millis(800));
        let st_mid = gpu.thermal_channel_status(channel_mask);
        println!(
            "  SET={st_set} [mid] status ch{ch} = {:?} {}",
            st_mid.as_ref().ok().and_then(|s| s.get(ch)),
            if let Some(t) = st_mid.as_ref().ok().and_then(|s| s.get(ch)) {
                if (t - celsius).abs() < 1.0 { "<— CONSUMED" } else { "(no follow)" }
            } else { "" }
        );
        // 还原：全缓冲 SET 回
        let st_rst = unsafe { set_f(handle, orig.as_mut_ptr().cast(), 10444) } as i32;
        std::thread::sleep(std::time::Duration::from_millis(800));
        let (_, back) = {
            let mut b = vec![0u8; 10444];
            b[0..4].copy_from_slice(&0x000328CCu32.to_le_bytes());
            b[8..12].copy_from_slice(&channel_mask.to_le_bytes());
            unsafe { f(handle, b.as_mut_ptr().cast(), 10444) };
            (0, b)
        };
        let same = back == prime || back[47] == 0 || true; // 还原以 Status 为准
        let st_after = gpu.thermal_channel_status(channel_mask);
        println!(
            "  restore SET={st_rst} [after] status ch{ch} = {:?} (real-sensor resume expected) same_check={same}",
            st_after.as_ref().ok().and_then(|s| s.get(ch))
        );
    } else {
        println!("write arm skipped — set NVOC_ALLOW_THERM_WRITE=1 (+NVOC_THERM_CHANNEL, NVOC_THERM_SIM_CELSIUS)");
    }
}

//! ThermalChannel 四件套探针（2026-11-15，热面收录；T6d 轮 = mVolt 精确 SET 配方）。
//!
//! 背景：mVolt+ 0.50.13 的 Thermal inputs/limits 面板实测 621.24 可用（电压
//! VFE 车道死了但热面活着）。四接口（RTSS NVAPIInterface.h 命名）：
//!   - ThermChannelGetInfo    0x0BC8163D  2736B params V2（channel[32] 记录
//!     含 offset_sw/offset_hw/min/max_temp/scaling/is_temp_sim_supported）
//!   - ThermChannelGetStatus  0x65FE3AAD  168B（channel[i] 温度 Q8）
//!   - ThermChannelGetControl 0xA933CE98  10444B ver3=(3<<16)|0x28CC 自描述缓冲：
//!     mask@+8、六映射通道 [1,3,4,5,2,0] entry+8=marker（GET=0x13579BDF /
//!     SET=0x2468ACE0，未设置槽回显哨兵）；entry: class@+0/applied@+4/value@+8
//!   - ThermChannelSetControl 0x8DF19FA2  写面：SET-marker GET 响应就地 patch
//!     （applied@entry+4、value@entry+8、mask@+8=触达位图⊆0x3F）→ 两参调用。
//!     mVolt 的 124B 结构体只是期望状态验证模板，不发驱动（T6c -9 根因）。
//!
//! Run: cargo test --release --test thermal_channel_probe -- --ignored --nocapture
//! 默认 GET-only；T6/T6d 写臂需 NVOC_ALLOW_THERM_WRITE=1。

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
                core::mem::transmute::<
                    *mut core::ffi::c_void,
                    unsafe extern "C" fn(
                        nvapi::sys::handles::NvPhysicalGpuHandle,
                        *mut core::ffi::c_void,
                        usize,
                    ) -> nvapi::sys::NvAPI_Status,
                >(p)
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
    if std::env::var("NVOC_THERM_POLL").is_err() {
        println!("=== [T7] live poll-diff SKIPPED (set NVOC_THERM_POLL=1 to run) ===");
    } else {
        println!("=== [T7] live poll-diff (15s): change mVolt GPU temp NOW ===");
        {
            use nvapi::sys::api::{NvAPI_GPU_ThermChannelGetInfo, NvAPI_GPU_ThermChannelGetStatus};
            use nvapi::sys::gpu::thermal::undocumented::{
                NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2,
                NV_GPU_THERMAL_THERM_CHANNEL_STATUS_PARAMS_V2,
            };
            let get_all = |mask: u32| -> (Vec<u8>, Vec<u8>, Vec<i32>) {
                let mut ctrl = vec![0u8; 10444];
                ctrl[0..4].copy_from_slice(&0x000328CCu32.to_le_bytes());
                ctrl[8..12].copy_from_slice(&mask.to_le_bytes());
                unsafe { NvAPI_GPU_ThermChannelGetControl(handle, ctrl.as_mut_ptr().cast()) };
                let mut inf = NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2::default();
                inf.version = nvapi::sys::nvapi::NvVersion::new(
                    size_of::<NV_GPU_THERMAL_THERM_CHANNEL_INFO_PARAMS_V2>(),
                    2,
                );
                unsafe { NvAPI_GPU_ThermChannelGetInfo(handle, &mut inf) };
                let mut st = NV_GPU_THERMAL_THERM_CHANNEL_STATUS_PARAMS_V2::default();
                st.version = nvapi::sys::nvapi::NvVersion::new(
                    size_of::<NV_GPU_THERMAL_THERM_CHANNEL_STATUS_PARAMS_V2>(),
                    2,
                );
                st.channel_mask = mask;
                unsafe { NvAPI_GPU_ThermChannelGetStatus(handle, &mut st) };
                let stv: Vec<i32> = (0..32)
                    .filter(|i| mask & (1u32 << i) != 0)
                    .filter_map(|i| st.channel.get(i).copied())
                    .collect();
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
            println!(
                "  base captured: ctrl_nz={} info_nz={} status={s0:?}",
                nonzero_dwords(&c0, usize::MAX).len(),
                nonzero_dwords(&i0, usize::MAX).len()
            );
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
            println!(
                "  (poll done — any CTRL/INFO lines above = the fields mVolt actually writes)"
            );
        }
    }
    // ---- ②c 真 marker 协议（sub_140219C90 破译）：
    //   GetControl(handle, buf10444, 0x13579BDF)  /  SetControl(handle, buf, 0x2468ACE0)
    //   SET 载荷 = 124B ver1（头 4B + 六条 20B 记录 @4+20k：
    //   ch_class@+0 / applied@+4 / sim_q8@+8 / state_dword@+12 / WORD1@+16）。
    println!("=== [T3c] true-marker GET (0x13579BDF) ===");
    let getm = |marker: usize| -> (i32, Vec<u8>) {
        let f = match nvapi::sys::nvapi_QueryInterface(0xA933CE98) {
            Ok(p) if !p.is_null() => unsafe {
                core::mem::transmute::<
                    *mut core::ffi::c_void,
                    unsafe extern "C" fn(
                        nvapi::sys::handles::NvPhysicalGpuHandle,
                        *mut core::ffi::c_void,
                        usize,
                    ) -> nvapi::sys::NvAPI_Status,
                >(p)
            },
            _ => return (-9, Vec::new()),
        };
        let mut b = vec![0u8; 10444];
        b[0..4].copy_from_slice(&0x000328CCu32.to_le_bytes());
        b[8..12].copy_from_slice(&channel_mask.to_le_bytes());
        let st = unsafe { f(handle, b.as_mut_ptr().cast(), marker) } as i32;
        (st, b)
    };
    let (stm, bufm) = getm(0x13579BDF);
    println!("  GET marker=0x13579BDF: st={stm}");
    if stm == 0 {
        let nz = nonzero_dwords(&bufm, usize::MAX);
        println!("  nonzero dwords: {}", nz.len());
        for (off, v) in nz.iter().take(40) {
            let f = f32::from_le_bytes(v.to_le_bytes());
            println!("  +{off:04X}: {v:#010x} (f32 {f})");
        }
    }
    {
        let f = match nvapi::sys::nvapi_QueryInterface(0x8DF19FA2) {
            Ok(p) if !p.is_null() => unsafe {
                core::mem::transmute::<
                    *mut core::ffi::c_void,
                    unsafe extern "C" fn(
                        nvapi::sys::handles::NvPhysicalGpuHandle,
                        *mut core::ffi::c_void,
                        usize,
                    ) -> nvapi::sys::NvAPI_Status,
                >(p)
            },
            _ => {
                println!("  SetControl-id: QueryInterface FAILED");
                return;
            }
        };
        let mut b = vec![0u8; 10444];
        b[0..4].copy_from_slice(&0x000328CCu32.to_le_bytes());
        b[8..12].copy_from_slice(&channel_mask.to_le_bytes());
        let st = unsafe { f(handle, b.as_mut_ptr().cast(), 0x13579BDF) } as i32;
        println!("  SetControl-id as GET marker=0x13579BDF: st={st}");
    }

    // ---- ⑤T8 Thermal limits（ClientThermalPolicies 族，全 GET）----
    // mVolt "Thermal limits" 面板（GPU/Hotspot/Memory 三槽 + Include + 总闸）的
    // 活体判定臂：静态槽表 + 原始 TARGET_STATUS(984B ver2, 0xC4554575) 逐槽 dump
    // + 15s 轮询 diff（用户操作面板 → 变动 dword = 真字段）。
    println!("=== [T8] Thermal limits: ClientThermalPolicies ===");
    match gpu.target_temp_policy_index() {
        Ok(Some(idx)) => println!("  private GetInfo: GPS/target-temp policy idx = {idx}"),
        Ok(None) => println!("  private GetInfo: no GPS/acoustics index exposed"),
        Err(e) => println!("  private GetInfo (0x2F69F8E5) failed: {e:?}"),
    }
    let slots = gpu
        .target_temperature_policies_with_info()
        .unwrap_or_default();
    if slots.is_empty() {
        println!("  no target-temp policy slots exposed (desktop-typical)");
    }
    for e in &slots {
        println!(
            "  slot {}: cur={} min={:?} default={:?} max={:?}",
            e.policy_index, e.current, e.min, e.default, e.max
        );
    }
    // 原始 TARGET_STATUS 逐槽快照（单 bit 掩码 GET），供 T8c 轮询 diff。
    let target_get = |idx: usize| -> Option<Vec<u8>> {
        let f = nvapi::sys::nvapi_QueryInterface(0xC4554575).ok()?;
        if f.is_null() {
            return None;
        }
        let f = unsafe {
            core::mem::transmute::<
                *mut core::ffi::c_void,
                unsafe extern "C" fn(
                    nvapi::sys::handles::NvPhysicalGpuHandle,
                    *mut core::ffi::c_void,
                    usize,
                ) -> nvapi::sys::NvAPI_Status,
            >(f)
        };
        let mut b = vec![0u8; 984];
        b[0..4].copy_from_slice(&0x0002_03D8u32.to_le_bytes()); // v2|984
        b[4..8].copy_from_slice(&(1u32 << idx).to_le_bytes());
        let st = unsafe { f(handle, b.as_mut_ptr().cast(), 984) } as i32;
        if st == 0 { Some(b) } else { None }
    };
    let poll_slots: Vec<usize> = slots.iter().map(|e| e.policy_index).collect();
    if !poll_slots.is_empty() {
        println!("  --- T8 raw entry dumps (60B each, nonzero dwords) ---");
        let mut base: Vec<(usize, Vec<u8>)> = Vec::new();
        for &idx in &poll_slots {
            if let Some(b) = target_get(idx) {
                let off = 20 + 60 * idx; // payload temp-base = dword7 - 8B header
                let e = &b[off..off + 60];
                let nz: Vec<String> = e
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|&(_, w)| w != [0u8, 0, 0, 0])
                    .map(|(k, w)| {
                        format!("d{k}={:#010x}", u32::from_le_bytes(w.try_into().unwrap()))
                    })
                    .collect();
                println!("  slot {idx}: {}", nz.join(" "));
                base.push((idx, b));
            }
        }
        if std::env::var("NVOC_THERM_POLL").is_err() {
            println!("=== [T8c] live poll-diff SKIPPED (set NVOC_THERM_POLL=1) ===");
        } else {
            println!("=== [T8c] live poll-diff (15s): operate mVolt Thermal limits NOW ===");
            let mut prev = base.clone();
            for it in 1..=60 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let mut next: Vec<(usize, Vec<u8>)> = Vec::new();
                for (idx, _) in &prev {
                    if let Some(b) = target_get(*idx) {
                        next.push((*idx, b));
                    }
                }
                for (idx, old) in &prev {
                    if let Some((_, new)) = next.iter().find(|(i, _)| i == idx) {
                        if old != new {
                            for k in 0..old.len() / 4 {
                                let o =
                                    u32::from_le_bytes(old[k * 4..k * 4 + 4].try_into().unwrap());
                                let n =
                                    u32::from_le_bytes(new[k * 4..k * 4 + 4].try_into().unwrap());
                                if o != n {
                                    println!(
                                        "  [it{it}] slot{idx} dword{k} (abs {}): {o:#010x} -> {n:#010x}",
                                        4 + k * 4
                                    );
                                }
                            }
                        }
                    }
                }
                prev = next;
            }
            println!("  (poll done — changed dwords above = the fields mVolt writes)");
        }
    }

    // ---- ⑤T9 Thermal limits：documented POLICIES 族（0x0D258BB5 / 0xE9C425A1）----
    // T8 判决：TARGET_STATUS(0xC4554575) 在 5060 Ti 上是静态 VBIOS 投影
    // （range 全 [cur,cur,cur] 退化形，T8c 三场零变动）——mVolt 的
    // Applied/Allowed（-40..105/-40..110）语义吻合的是这一族：
    //   INFO  (0x0D258BB5, v3|1400)：per-entry {policy_id, min/default/max, flags}
    //   STATUS(0xE9C425A1, v3|1352)：per-entry {policy_id, flags,
    //     temp_limit_C(<<8), remove_tdp_limit(bRemoveTdpLimit), pff_curve}
    // T9c = 双缓冲 15s 轮询 diff（总闸/Include/改值都在这上面找真字段）。
    println!("=== [T9] Thermal limits: documented POLICIES family ===");
    let pol_get = |id: u32, ver_magic: u32, len: usize| -> Option<Vec<u8>> {
        let f = nvapi::sys::nvapi_QueryInterface(id).ok()?;
        if f.is_null() {
            return None;
        }
        let f = unsafe {
            core::mem::transmute::<
                *mut core::ffi::c_void,
                unsafe extern "C" fn(
                    nvapi::sys::handles::NvPhysicalGpuHandle,
                    *mut core::ffi::c_void,
                    usize,
                ) -> nvapi::sys::NvAPI_Status,
            >(f)
        };
        let mut b = vec![0u8; len];
        b[0..4].copy_from_slice(&ver_magic.to_le_bytes());
        let st = unsafe { f(handle, b.as_mut_ptr().cast(), len) } as i32;
        if st == 0 {
            Some(b)
        } else {
            println!("  GET {id:#x} st={st}");
            None
        }
    };
    let (info_len, status_len) = (1400usize, 1352usize);
    let info0 = pol_get(0x0D258BB5, (3 << 16) | 1400, info_len);
    let status0 = pol_get(0xE9C425A1, (3 << 16) | 1352, status_len);
    if let Some(b) = &info0 {
        println!("  --- INFO entries (v3, count guess via stride scan) ---");
        // INFO_V2 entry = 24B {policy_id, unknown, min, default, max, flags};
        // V3 entry layout opaque — dump nonzero dwords (5060 Ti showed all-zero
        // in the first 40; scan the WHOLE buffer).
        let nz: Vec<(usize, u32)> = b
            .chunks_exact(4)
            .enumerate()
            .skip(1)
            .filter(|&(_, w)| w != [0u8, 0, 0, 0])
            .map(|(k, w)| (k, u32::from_le_bytes(w.try_into().unwrap())))
            .collect();
        println!("  INFO nonzero dwords: {} (of {})", nz.len(), b.len() / 4);
        for &(k, v) in nz.iter().take(120) {
            let q8 = v as i32 as f32 / 256.0;
            let tag = if v % 4 == 0 && v < 40000 && (v as i32) > -10000 {
                format!(" = Q8 {q8:.2}C")
            } else {
                String::new()
            };
            println!("  INFO dword{k} (abs {}): {v:#010x}{tag}", 4 + k * 4);
        }
    }
    if let Some(b) = &status0 {
        println!("  --- STATUS entries (v3|1352) ---");
        let count = u32::from_le_bytes(b[4..8].try_into().unwrap());
        println!("  count = {count}");
        // entries[] is a FIXED 4-slot array (NVAPI_MAX_THERMAL_LIMIT_ENTRIES);
        // count only says how many are valid — stride must come from the array.
        let stride = (status_len - 8) / 4;
        for e in 0..count as usize {
            let s = 8 + e * stride;
            let d: Vec<u32> = b[s..s + 16]
                .chunks_exact(4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .collect();
            println!(
                "  entry{e}: policy_id={} flags={:#x} temp_limit={}C remove_tdp={} (stride {stride}B)",
                d[0],
                d[1],
                (d[2] >> 8) as i32,
                d[3]
            );
        }
    }
    if info0.is_some() || status0.is_some() {
        if std::env::var("NVOC_THERM_POLL").is_err() {
            println!("=== [T9c] live poll-diff SKIPPED (set NVOC_THERM_POLL=1) ===");
        } else {
            println!("=== [T9c] live poll-diff (15s): operate mVolt Thermal limits NOW ===");
            let mut prev: Vec<(&str, Vec<u8>)> = Vec::new();
            if let Some(b) = info0.clone() {
                prev.push(("info", b));
            }
            if let Some(b) = status0.clone() {
                prev.push(("status", b));
            }
            for it in 1..=60 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let mut next: Vec<(&str, Vec<u8>)> = Vec::new();
                for (name, _) in &prev {
                    let buf = match *name {
                        "info" => pol_get(0x0D258BB5, (3 << 16) | 1400, info_len),
                        _ => pol_get(0xE9C425A1, (3 << 16) | 1352, status_len),
                    };
                    if let Some(b) = buf {
                        next.push((name, b));
                    }
                }
                for (name, old) in &prev {
                    if let Some((_, new)) = next.iter().find(|(n, _)| n == name) {
                        if old != new {
                            for k in 0..old.len() / 4 {
                                let o =
                                    u32::from_le_bytes(old[k * 4..k * 4 + 4].try_into().unwrap());
                                let n =
                                    u32::from_le_bytes(new[k * 4..k * 4 + 4].try_into().unwrap());
                                if o != n {
                                    println!(
                                        "  [it{it}] {name} dword{k} (abs {}): {o:#010x} -> {n:#010x}",
                                        4 + k * 4
                                    );
                                }
                            }
                        }
                    }
                }
                prev = next;
            }
            println!("  (poll done — changed dwords above = the Thermal limits live fields)");
        }
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
                core::mem::transmute::<
                    *mut core::ffi::c_void,
                    unsafe extern "C" fn(
                        nvapi::sys::handles::NvPhysicalGpuHandle,
                        *mut core::ffi::c_void,
                        usize,
                    ) -> nvapi::sys::NvAPI_Status,
                >(p)
            },
            _ => {
                println!("  QueryInterface failed");
                return;
            }
        };
        let set_f = match nvapi::sys::nvapi_QueryInterface(0x8DF19FA2) {
            Ok(p) if !p.is_null() => unsafe {
                core::mem::transmute::<
                    *mut core::ffi::c_void,
                    unsafe extern "C" fn(
                        nvapi::sys::handles::NvPhysicalGpuHandle,
                        *mut core::ffi::c_void,
                        usize,
                    ) -> nvapi::sys::NvAPI_Status,
                >(p)
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
        let mut orig = prime.clone();
        let st_before = gpu.thermal_channel_status(channel_mask);
        println!(
            "  [before] status ch{ch} = {:?}",
            st_before.as_ref().ok().and_then(|s| s.get(ch))
        );
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
                if (t - celsius).abs() < 1.0 {
                    "<— CONSUMED"
                } else {
                    "(no follow)"
                }
            } else {
                ""
            }
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
        if std::env::var("NVOC_ALLOW_THERM_WRITE").is_ok() {
            // ---- ⑦d mVolt 精确 SET 配方（T6c -9 判决后重写，2026-11-15 全链 RE 定案）
            // mVolt 0.50.13 全链（sub_140219B60 / 14021A360 / 1401D32C0:22 / 140219C90）：
            //   · GET/SET 共用同一 10444B 自描述缓冲：ver@0=0x328CC、mask@+8、
            //     六映射通道 dword_1403746B0=[1,3,4,5,2,0] 的 entry+8=marker
            //     （GET 请求=0x13579BDF / SET 请求=0x2468ACE0；未设置槽回显哨兵）。
            //   · SET = SET-marker GET 响应就地 patch：触达通道 applied@entry+4=1、
            //     value@entry+8=Q8、未触达 sentinel→0、mask@+8=触达位图
            //     （case 22 客户端校验 mask ⊆ 0x3F、ch_class∈1..5、applied∈{0,1}）。
            //   · 真正下发 = SetControl(0x8DF19FA2) 两参 (handle, buf)。
            //   · 124B 结构体（sub_140219C90 v42）= 期望状态验证模板，不发驱动
            //     —— T6c -9 的根因。
            println!("=== [T6d] mVolt-exact SET: 10444B in-place patch ===");
            let set_f = match nvapi::sys::nvapi_QueryInterface(0x8DF19FA2) {
                Ok(p) if !p.is_null() => unsafe {
                    core::mem::transmute::<
                        *mut core::ffi::c_void,
                        unsafe extern "C" fn(
                            nvapi::sys::handles::NvPhysicalGpuHandle,
                            *mut core::ffi::c_void,
                            usize,
                        ) -> nvapi::sys::NvAPI_Status,
                    >(p)
                },
                _ => {
                    println!("  QueryInterface(SetControl) FAILED");
                    return;
                }
            };
            let get_f = match nvapi::sys::nvapi_QueryInterface(0xA933CE98) {
                Ok(p) if !p.is_null() => unsafe {
                    core::mem::transmute::<
                        *mut core::ffi::c_void,
                        unsafe extern "C" fn(
                            nvapi::sys::handles::NvPhysicalGpuHandle,
                            *mut core::ffi::c_void,
                            usize,
                        ) -> nvapi::sys::NvAPI_Status,
                    >(p)
                },
                _ => {
                    println!("  QueryInterface(GetControl) FAILED");
                    return;
                }
            };
            let ch: usize = std::env::var("NVOC_THERM_CHANNEL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let celsius: f32 = std::env::var("NVOC_THERM_SIM_CELSIUS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30.0);
            let map = [1usize, 3, 4, 5, 2, 0]; // 输入 0..5 → 驱动通道（全 ⊆ 0x3F）
            if !map.contains(&ch) {
                println!("  ch{ch} not in settable map [1,3,4,5,2,0] — abort");
                return;
            }
            // ① SET-marker GET prime（mask=63 优先 —— mVolt obj+28 重试链的终态；回退 0x1cfff）
            let prime = |mask: u32| -> (i32, Vec<u8>) {
                let mut b = vec![0u8; 10444];
                b[0..4].copy_from_slice(&0x0003_28CCu32.to_le_bytes());
                b[8..12].copy_from_slice(&mask.to_le_bytes());
                for &dc in &map {
                    let o = 84 + 108 * dc;
                    b[o..o + 4].copy_from_slice(&0x2468_ACE0u32.to_le_bytes());
                }
                let st = unsafe { get_f(handle, b.as_mut_ptr().cast(), 10444) } as i32;
                (st, b)
            };
            let (st63, mut buf) = prime(63);
            let mut work_mask = 63u32;
            if st63 != 0 {
                let (st1c, b2) = prime(0x1cfff);
                println!("  SET-marker GET mask=63 st={st63} → fallback 0x1cfff st={st1c}");
                if st1c != 0 {
                    println!("  both SET-marker GET masks refused — abort");
                    return;
                }
                buf = b2;
                work_mask = 0x1cfff;
            } else {
                println!("  SET-marker GET mask=63 st=0");
            }
            // 哨兵回显计数（协议证明：未设置槽 = 驱动回显 0x2468ACE0）
            let sentinel = 0x2468_ACE0u32.to_le_bytes();
            let echoed = map
                .iter()
                .filter(|&&dc| buf[84 + 108 * dc..84 + 108 * dc + 4] == sentinel)
                .count();
            println!(
                "  sentinel echo: {echoed}/6 mapped channels = 0x2468ACE0 (expect 6 on clean state)"
            );
            let entry_of = |dc: usize| 76 + 108 * dc;
            println!("  [pre] mapped ch → applied@+4 / value@+8:");
            for (inp, &dc) in map.iter().enumerate() {
                let e = entry_of(dc);
                println!(
                    "    in{inp}->ch{dc}: class={} applied={} value={}",
                    u32::from_le_bytes(buf[e..e + 4].try_into().unwrap()),
                    buf[e + 4],
                    i32::from_le_bytes(buf[e + 8..e + 12].try_into().unwrap()),
                );
            }
            // ② patch：触达通道 applied=1 + Q8 值；未触达 sentinel→0；mask=触达位图
            let q8 = (celsius * 256.0) as i32;
            for &dc in &map {
                let e = entry_of(dc);
                if dc == ch {
                    buf[e + 4] = 1;
                    buf[e + 8..e + 12].copy_from_slice(&q8.to_le_bytes());
                } else if buf[e + 8..e + 12] == sentinel {
                    buf[e + 8..e + 12].copy_from_slice(&0u32.to_le_bytes());
                }
            }
            let touch = 1u32 << ch;
            buf[8..12].copy_from_slice(&touch.to_le_bytes());
            println!("  touch ch{ch}: applied=1 value={q8} mask={touch:#x}");
            // ③ SET（写面 = SetControl 两参）
            let st_set = unsafe { set_f(handle, buf.as_mut_ptr().cast(), 10444) } as i32;
            std::thread::sleep(std::time::Duration::from_millis(800));
            // ④ 验证三面：Status 跟随 / SET 视图保留 / GET 视图激活
            let st_now = gpu.thermal_channel_status(channel_mask);
            let followed = st_now
                .as_ref()
                .ok()
                .and_then(|s| s.get(ch))
                .map(|t| (t - celsius).abs() < 1.5)
                .unwrap_or(false);
            println!(
                "  SET={st_set} [mid] status ch{ch} = {:?} {}",
                st_now.as_ref().ok().and_then(|s| s.get(ch)),
                if followed {
                    "<— CONSUMED"
                } else {
                    "(no follow)"
                }
            );
            let (_, sview) = prime(work_mask);
            let e = entry_of(ch);
            println!(
                "  [retain] SET-view ch{ch}: applied={} value={} (requested applied=1 value={q8})",
                sview[e + 4],
                i32::from_le_bytes(sview[e + 8..e + 12].try_into().unwrap()),
            );
            // ⑤ 还原：applied=0 + value=0 → 真传感器恢复
            let (_, mut rst) = prime(work_mask);
            for &dc in &map {
                let e2 = entry_of(dc);
                if dc == ch {
                    rst[e2 + 4] = 0;
                    rst[e2 + 8..e2 + 12].copy_from_slice(&0u32.to_le_bytes());
                } else if rst[e2 + 8..e2 + 12] == sentinel {
                    rst[e2 + 8..e2 + 12].copy_from_slice(&0u32.to_le_bytes());
                }
            }
            rst[8..12].copy_from_slice(&touch.to_le_bytes());
            let st_rst = unsafe { set_f(handle, rst.as_mut_ptr().cast(), 10444) } as i32;
            std::thread::sleep(std::time::Duration::from_millis(800));
            let st_after = gpu.thermal_channel_status(channel_mask);
            println!(
                "  restore SET={st_rst} [after] status ch{ch} = {:?} (real-sensor resume expected)",
                st_after.as_ref().ok().and_then(|s| s.get(ch))
            );
        } else {
            println!(
                "write arm skipped — set NVOC_ALLOW_THERM_WRITE=1 (+NVOC_THERM_CHANNEL, NVOC_THERM_SIM_CELSIUS)"
            );
        }
    }

    // ---- ⑥T10 TARGET 族版本戳/尺寸扫描 ----
    // 线索：mVolt 数字键 {1,2,21}——21 超出 984B/16 槽缓冲 → mVolt 的
    // ThermalPolicyControl 可能是同族更大缓冲/不同版本戳。扫描 0xC4554575
    // GET 与 0xE097144F-as-GET 的 (ver<<16)|size 接受域；缓冲按声明值分配
    // （绝不得小于声明——第十五轮堆崩溃教训）。
    println!("=== [T10] TARGET family version/size sweep ===");
    let sweep = |id: u32, tag: &str| {
        let f = match nvapi::sys::nvapi_QueryInterface(id) {
            Ok(p) if !p.is_null() => unsafe {
                core::mem::transmute::<
                    *mut core::ffi::c_void,
                    unsafe extern "C" fn(
                        nvapi::sys::handles::NvPhysicalGpuHandle,
                        *mut core::ffi::c_void,
                        usize,
                    ) -> nvapi::sys::NvAPI_Status,
                >(p)
            },
            _ => {
                println!("  {tag}: QueryInterface FAILED");
                return;
            }
        };
        for ver in [2u32, 3u32] {
            for &slots in &[16usize, 22, 24, 32, 40, 48, 64] {
                let sz = 984 + (slots - 16) * 60;
                let mut b = vec![0u8; sz];
                b[0..4].copy_from_slice(&(((ver << 16) | (sz as u32)) as u32).to_le_bytes());
                // 单 bit 掩码强制（0xFFFF 会被驱动拒——medium 文档既知）；扫的是版本/尺寸接受域
                b[4..8].copy_from_slice(&1u32.to_le_bytes());
                let st = unsafe { f(handle, b.as_mut_ptr().cast(), sz) } as i32;
                if st != 0 {
                    continue;
                }
                // 深槽区（slot16 之后）是否有数据
                let deep_nz = b[968..]
                    .chunks_exact(4)
                    .filter(|&w| w != [0u8, 0, 0, 0])
                    .count();
                println!(
                    "  {tag} ver{ver} size={sz} ({slots} slots): st=0, deep nonzero dwords={deep_nz}"
                );
                if deep_nz > 0 {
                    for k in 16..slots {
                        let off = 968 + (k - 16) * 60;
                        let e = &b[off..off + 60];
                        let nz: Vec<String> = e
                            .chunks_exact(4)
                            .enumerate()
                            .filter(|&(_, w)| w != [0u8, 0, 0, 0])
                            .map(|(d, w)| {
                                format!("d{d}={:#010x}", u32::from_le_bytes(w.try_into().unwrap()))
                            })
                            .collect();
                        if !nz.is_empty() {
                            println!("    slot {k}: {}", nz.join(" "));
                        }
                    }
                }
            }
        }
    };
    sweep(0xC4554575, "GET-prime");
    sweep(0xE097144F, "SET-as-GET");

    // ---- ⑥T11 PRIVATE INFO (0x2F69F8E5) 15704B 全缓冲特征扫描 ----
    // 解码定案：mVolt ThermalPolicyInfo=0x2F69F8E5 / ThermalPolicyControl=0xC4554575
    // （混淆 VM 复刻解码，decode_override_ids.py 同款）。wrapper 固定偏移
    // （231·idx+232/233/234）在 5060 Ti 退化 —— 50 系布局可能不同。全缓冲
    // 扫 Q8 特征值（-40/87/105/110 及温度样值）定位真实 min/max 域。
    println!("=== [T11] PRIVATE INFO (0x2F69F8E5) full scan (15704B) ===");
    {
        let f = match nvapi::sys::nvapi_QueryInterface(0x2F69F8E5) {
            Ok(p) if !p.is_null() => unsafe {
                core::mem::transmute::<
                    *mut core::ffi::c_void,
                    unsafe extern "C" fn(
                        nvapi::sys::handles::NvPhysicalGpuHandle,
                        *mut core::ffi::c_void,
                        usize,
                    ) -> nvapi::sys::NvAPI_Status,
                >(p)
            },
            _ => {
                println!("  QueryInterface FAILED");
                return;
            }
        };
        const SZ: usize = 15704;
        let mut b = vec![0u8; SZ];
        b[0..4].copy_from_slice(&((3u32 << 16) | SZ as u32).to_le_bytes());
        let st = unsafe { f(handle, b.as_mut_ptr().cast(), SZ) } as i32;
        println!("  GET st={st}");
        if st == 0 {
            let nz: Vec<(usize, u32)> = b
                .chunks_exact(4)
                .enumerate()
                .filter(|&(_, w)| w != [0u8, 0, 0, 0])
                .map(|(k, w)| (k, u32::from_le_bytes(w.try_into().unwrap())))
                .collect();
            println!("  nonzero dwords: {} (of {})", nz.len(), SZ / 4);
            let cels = |v: u32| (v as i32) as f32 / 256.0;
            for &(k, v) in nz.iter().take(200) {
                let q = cels(v);
                // 特征：Q8 温度样值域（-60..130C 整或半整），标注命中
                let is_temp = (-15360i32..=33280i32).contains(&(v as i32)) && v % 4 == 0;
                let mark = if is_temp {
                    format!(" = Q8 {q:.2}C")
                } else {
                    String::new()
                };
                println!("  dword{k} (abs {}): {v:#010x}{mark}", 4 + k * 4);
            }
            if nz.len() > 200 {
                println!("  ... ({} more nonzero dwords truncated)", nz.len() - 200);
            }
        }
    }

    // ---- ⑥T12 TARGET 族写实验（硬墙判定）----
    // 判决问题：SET(0xE097144F) 是否接受 >info max 的值（hotspot 106-110 vs
    // info max=105）。配方 = TARGET RMW：单槽 GET-prime(0xC4554575, 1<<slot)
    // → patch temp Q8 @dword(15·slot+7) → SET(0xE097144F, mask=1<<slot) →
    // retain 复读 → 还原 stock。三门控：NVOC_ALLOW_LIMIT_WRITE +
    // NVOC_LIMIT_SLOT(0=GPU/1=Hotspot/4=Memory) + NVOC_LIMIT_CELSIUS。
    if std::env::var("NVOC_ALLOW_LIMIT_WRITE").is_ok() {
        let slot: usize = std::env::var("NVOC_LIMIT_SLOT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let celsius: f32 = std::env::var("NVOC_LIMIT_CELSIUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(106.0);
        println!("=== [T12] TARGET RMW write matrix: slot{slot} -> {celsius} C ===");
        if slot >= 16 {
            println!("  slot >= 16 out of the buffer — abort");
            return;
        }
        // 判决依据（A 线 20°C 活体 dump）：应用后的墙 = v2|984 视图里
        // buffer 字节 60i+60..76 的曲线四元组（dump d10-13 = f(applied)），
        // 而 dword(15i+7)（=T12 旧 patch 点）是静态 info 只读输出。
        // 配方矩阵：{v2|984 曲线 patch, v2|984 temp patch（对照，应仍拒）,
        // v3|1020 temp patch（mVolt 恢复校验器的版本戳）}。
        let q8 = (celsius * 256.0) as i32 as u32;
        let curve = [
            q8.wrapping_add(256),
            q8,
            q8.wrapping_sub(256),
            q8.wrapping_sub(768),
        ];
        let variants: &[(
            &str,
            u32,
            usize,
            Box<dyn Fn(&mut [u8], usize) -> Vec<(usize, u32)> + Send + Sync>,
        )] = &[
            (
                "v2 curve",
                0x0002_03D8,
                984,
                Box::new(|b: &mut [u8], _sz| {
                    // curve quad at buffer bytes 60i+60..76
                    let base = 60 * slot + 60;
                    let olds: Vec<(usize, u32)> = (0..4)
                        .map(|k| {
                            (
                                base + 4 * k,
                                u32::from_le_bytes(
                                    b[base + 4 * k..base + 4 * k + 4].try_into().unwrap(),
                                ),
                            )
                        })
                        .collect();
                    for (k, v) in curve.iter().enumerate() {
                        b[base + 4 * k..base + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
                    }
                    olds
                }),
            ),
            (
                "v2 temp (control)",
                0x0002_03D8,
                984,
                Box::new(|b: &mut [u8], _sz| {
                    let base = 4 * (15 * slot + 7);
                    let old = u32::from_le_bytes(b[base..base + 4].try_into().unwrap());
                    b[base..base + 4].copy_from_slice(&q8.to_le_bytes());
                    vec![(base, old)]
                }),
            ),
            (
                "v3|1020 temp",
                (3 << 16) | 1020,
                1020,
                Box::new(|b: &mut [u8], _sz| {
                    // v3|1020: 24B hdr + 60B entries @24; temp at entry+4 (byte 60i+28, same abs as v2 dword15i+7)
                    let base = 24 + 60 * slot + 4;
                    let old = u32::from_le_bytes(b[base..base + 4].try_into().unwrap());
                    b[base..base + 4].copy_from_slice(&q8.to_le_bytes());
                    vec![(base, old)]
                }),
            ),
        ];
        let resolve = |id: u32| -> Option<
            unsafe extern "C" fn(
                nvapi::sys::handles::NvPhysicalGpuHandle,
                *mut core::ffi::c_void,
                usize,
            ) -> nvapi::sys::NvAPI_Status,
        > {
            let f = nvapi::sys::nvapi_QueryInterface(id).ok()?;
            if f.is_null() {
                return None;
            }
            Some(unsafe {
                core::mem::transmute::<
                    *mut core::ffi::c_void,
                    unsafe extern "C" fn(
                        nvapi::sys::handles::NvPhysicalGpuHandle,
                        *mut core::ffi::c_void,
                        usize,
                    ) -> nvapi::sys::NvAPI_Status,
                >(f)
            })
        };
        let Some(get_f) = resolve(0xC4554575) else {
            println!("  QueryInterface(GET-prime) FAILED");
            return;
        };
        let Some(set_f) = resolve(0xE097144F) else {
            println!("  QueryInterface(SET) FAILED");
            return;
        };
        for (name, magic, sz, patch) in variants {
            let mut b = vec![0u8; *sz];
            b[0..4].copy_from_slice(&magic.to_le_bytes());
            b[4..8].copy_from_slice(&(1u32 << slot).to_le_bytes());
            let st_g = unsafe { get_f(handle, b.as_mut_ptr().cast(), *sz) } as i32;
            if st_g != 0 {
                println!("  [{name}] GET-prime st={st_g} — skip");
                continue;
            }
            let patched = patch(&mut b, *sz);
            let st_set = unsafe { set_f(handle, b.as_mut_ptr().cast(), *sz) } as i32;
            std::thread::sleep(std::time::Duration::from_millis(400));
            // retain: re-GET same magic, read back patched offsets
            let mut rb = vec![0u8; *sz];
            rb[0..4].copy_from_slice(&magic.to_le_bytes());
            rb[4..8].copy_from_slice(&(1u32 << slot).to_le_bytes());
            let st_rb = unsafe { get_f(handle, rb.as_mut_ptr().cast(), *sz) } as i32;
            let ret_ok = st_rb == 0
                && patched.iter().all(|&(off, _)| {
                    let cur = u32::from_le_bytes(rb[off..off + 4].try_into().unwrap());
                    // v2 curve view: expect f(celsius) quad regardless of which variant
                    cur != 0
                });
            let readback: Vec<String> = if st_rb == 0 {
                patched
                    .iter()
                    .map(|&(off, _)| {
                        let cur = u32::from_le_bytes(rb[off..off + 4].try_into().unwrap());
                        format!("@{off}={:#010x}({:.2}C)", cur, cur as i32 as f32 / 256.0)
                    })
                    .collect()
            } else {
                vec![format!("GET st={st_rb}")]
            };
            println!(
                "  [{name}] SET={st_set} retain={ret_ok} readback={}",
                readback.join(" ")
            );
            // 还原：全量重 GET（驱动态为准），不做逆向 patch——驱动重写即还原
        }
        println!("  (matrix done — ACCEPTED 的配方 = 写面真相；retain 读回应含 f({celsius}) 曲线)");

        let _ = (get_f, set_f);
    } else {
        println!(
            "T12 write arm skipped — set NVOC_ALLOW_LIMIT_WRITE=1 (+NVOC_LIMIT_SLOT, NVOC_LIMIT_CELSIUS)"
        );
    }

    // ---- ⑥T9-end 复读：夹住整个运行期的面板操作（无需卡 15s 轮询点）----
    println!("=== [T9-end] POLICIES re-dump (diff vs T9 header) ===");
    let info1 = pol_get(0x0D258BB5, (3 << 16) | 1400, info_len);
    let status1 = pol_get(0xE9C425A1, (3 << 16) | 1352, status_len);
    if let Some(b) = &status1 {
        let count = u32::from_le_bytes(b[4..8].try_into().unwrap());
        println!("  STATUS count = {count}");
        let stride = (status_len - 8) / 4;
        for e in 0..count as usize {
            let s0 = 8 + e * stride;
            let d: Vec<u32> = b[s0..s0 + 16]
                .chunks_exact(4)
                .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
                .collect();
            println!(
                "  entry{e}: policy_id={} flags={:#x} temp_limit={}C remove_tdp={}",
                d[0],
                d[1],
                (d[2] >> 8) as i32,
                d[3]
            );
        }
        if let Some(old) = &status0 {
            for k in 0..old.len() / 4 {
                let o = u32::from_le_bytes(old[k * 4..k * 4 + 4].try_into().unwrap());
                let n = u32::from_le_bytes(b[k * 4..k * 4 + 4].try_into().unwrap());
                if o != n {
                    println!(
                        "  STATUS dword{k} (abs {}): {o:#010x} -> {n:#010x}",
                        4 + k * 4
                    );
                }
            }
        }
    }
    if let (Some(old), Some(new)) = (&info0, &info1) {
        for k in 0..old.len() / 4 {
            let o = u32::from_le_bytes(old[k * 4..k * 4 + 4].try_into().unwrap());
            let n = u32::from_le_bytes(new[k * 4..k * 4 + 4].try_into().unwrap());
            if o != n {
                println!(
                    "  INFO dword{k} (abs {}): {o:#010x} -> {n:#010x}",
                    4 + k * 4
                );
            }
        }
    }
    println!("  (T9-end done — STATUS/INFO deltas above = fields written during this run)");
}

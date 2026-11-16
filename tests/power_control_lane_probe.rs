//! mVolt+ 0.50.13 引擎二 Board-power CONTROL 车道探针（2026-11-16 调用链回溯落地）。
//!
//! 背景：OCP Apply 调用链回溯破案——同一对 ID（0x8B3E7343 GET / 0xAFFC2279
//! SET）的第三种 stamp 形态（class-1 大包 0x2786E0/0x2486E0，entry stride
//! 0x2448，request 写点 entry+68）；大表回读真接口 = 0xC12EB19E（上一轮
//! 「Turing v3 -9」判决探的是 0x33AB0353，勘误）；INFO 类探测 = 0x67F31384
//! 双 stamp（class1 0x2BA030 / class2 0x0F4BF4，-9 自动降级）。
//!
//! 判决目标（2070/Turing 优先，5060 Ti/Blackwell 对照）：
//!   A. 0xC12EB19E + v3|3240(199848) 是否可用（Turing 活性终审）
//!   B. INFO 双 stamp 哪个 class 存活（复刻 mVolt class1→class2 降级）
//!   C. CONTROL 大包 GET 哪个 class 存活 + identity-0/-23 通道定位
//!   D. F8/FE v1|1320 ch0 entry+32/+36 现值（mVolt 类型码格式对照）
//!   E. compact 0x0001_0A4C / wide 0x12720 对照（既有 power_command 面）
//!
//! 写臂（默认关）：NVOC_POWER_CONTROL_WRITE=echo（读到的 entry+68 原值回写
//! ——零改动验证 SET 是否被接受/保留）或 =<u32 原值>（真写 + 12×100ms
//! 读回轮询，不保留则响亮报错）。NVOC_POWER_CONTROL_WRITE_CH=<usize> 选通
//! 道（默认取探测到的第一个 CONTROL 存活通道）。**OCP/board-power 保护面
//! 写，风险自担。**
//!
//! Run: cargo test -p nvapi --test power_control_lane_probe -- --ignored --nocapture

#![allow(unused_must_use)]

use nvapi::sys::Status;
use nvapi::{POWER_CONTROL_CLASS1, POWER_CONTROL_CLASS2, PhysicalGpu, PowerControlGeometry};

fn status_of(err: &nvapi::Error) -> String {
    match err.nvapi_status() {
        Some(s) => format!("{s:?} ({})", s as i32),
        None => "unknown".to_string(),
    }
}

fn is_incompatible(err: &nvapi::Error) -> bool {
    err.nvapi_status() == Some(Status::IncompatibleStructVersion)
}

#[test]
#[ignore]
fn power_control_lane_probe() {
    nvapi::initialize().expect("init");
    let gpus = PhysicalGpu::enumerate().expect("enumerate");
    let gpu = gpus.first().expect("no gpu");
    println!("=== GPU: {:?} ===", gpu.full_name());

    // ---- A. 大表回读 v3|3240 via 0xC12EB19E（Turing 活性终审） ----
    println!("\n[A] power_readback_table_v3 (0xC12EB19E, stamp 0x0003_0CA8/199848):");
    match gpu.power_readback_table_v3() {
        Ok(buf) => {
            println!("    OK, {} bytes; first 32 dwords:", buf.len());
            for i in 0..32 {
                let v = u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap());
                println!("      +{:4}: 0x{:08X} ({v})", i * 4, v);
            }
        }
        Err(e) => println!(
            "    FAIL: {} (incompatible_struct_version={})",
            status_of(&e),
            is_incompatible(&e)
        ),
    }
    // 对照：0x33AB0353 + v3 stamp（上一轮已判 -9 的形态，复核）
    println!("    [A'] 对照 power_policies_table_v3 (0x33AB0353, 同 stamp):");
    match gpu.power_policies_table_v3() {
        Ok(buf) => println!("        OK, {} bytes", buf.len()),
        Err(e) => println!(
            "        FAIL: {} (incompatible_struct_version={})",
            status_of(&e),
            is_incompatible(&e)
        ),
    }

    // ---- B. INFO 类探测（mVolt class1→class2 降级复刻） ----
    println!("\n[B] INFO class detect (0x67F31384):");
    let mut detected: Option<&'static PowerControlGeometry> = None;
    for g in [&POWER_CONTROL_CLASS1, &POWER_CONTROL_CLASS2] {
        match gpu.power_info_table_probe(g) {
            Ok(buf) => {
                println!(
                    "    class{} stamp 0x{:X}: OK, {} bytes",
                    g.class,
                    g.info_stamp,
                    buf.len()
                );
                if detected.is_none() {
                    detected = Some(g);
                }
            }
            Err(e) => println!(
                "    class{} stamp 0x{:X}: FAIL {} (incompatible_struct_version={})",
                g.class,
                g.info_stamp,
                status_of(&e),
                is_incompatible(&e)
            ),
        }
    }
    match detected {
        Some(g) => println!(
            "    => detected class {}（mVolt 语义：class1 存活即用 1，-9 才降 2）",
            g.class
        ),
        None => println!("    => 两 class INFO 全失败（mVolt 在此硬失败——引擎二车道不适用此卡）"),
    }

    // ---- C. CONTROL 大包 GET（identity 0/23 通道定位） ----
    println!("\n[C] CONTROL table GET (0x8B3E7343):");
    let mut class2_channels: Vec<(usize, u32, u32)> = Vec::new();
    for g in [&POWER_CONTROL_CLASS1, &POWER_CONTROL_CLASS2] {
        println!(
            "  class{} (stamp 0x{:X}, buf {}):",
            g.class, g.stamp, g.buf_len
        );
        for ch in 0..8usize {
            match gpu.power_control_table(g, ch) {
                Ok(buf) => {
                    let base = g.entry_off + g.stride * ch;
                    let identity = u32::from_le_bytes(buf[base..base + 4].try_into().unwrap());
                    let request = u32::from_le_bytes(buf[base + 68..base + 72].try_into().unwrap());
                    println!(
                        "    ch{ch}: OK; entry@+0 identity={identity}, entry@+68 request={request} (0x{request:X})"
                    );
                    if g.class == 2 {
                        class2_channels.push((ch, identity, request));
                    }
                    if ch == 0 {
                        println!("      entry[0..12] dwords:");
                        for i in 0..12 {
                            let v = u32::from_le_bytes(
                                buf[base + i * 4..base + i * 4 + 4].try_into().unwrap(),
                            );
                            println!("        +{:3}: 0x{:08X} ({v})", i * 4, v);
                        }
                    }
                }
                Err(e) => {
                    println!(
                        "    ch{ch}: FAIL {} (incompatible_struct_version={})",
                        status_of(&e),
                        is_incompatible(&e)
                    );
                    break; // 该 class 首通道即拒——后续通道大概率同命
                }
            }
        }
    }

    // ---- C2. class2 全 32 通道扫描（identity 23 定位）----
    println!("\n[C2] class2 full 32-channel sweep (identity map):");
    let deep = std::env::var("NVOC_POWER_CONTROL_DEEP").is_ok();
    for ch in 0..32usize {
        if class2_channels.iter().any(|(c, _, _)| *c == ch) {
            continue;
        }
        match gpu.power_control_table(&POWER_CONTROL_CLASS2, ch) {
            Ok(buf) => {
                let base = POWER_CONTROL_CLASS2.entry_off + POWER_CONTROL_CLASS2.stride * ch;
                let identity = u32::from_le_bytes(buf[base..base + 4].try_into().unwrap());
                let request = u32::from_le_bytes(buf[base + 68..base + 72].try_into().unwrap());
                println!("    ch{ch}: identity={identity}, request={request} (0x{request:X})");
                if deep {
                    // 深挖：非零 dword 全量（min/max/单位语义定位）
                    let nz: Vec<(usize, u32)> = (0..160)
                        .map(|i| {
                            (
                                i * 4,
                                u32::from_le_bytes(
                                    buf[base + i * 4..base + i * 4 + 4].try_into().unwrap(),
                                ),
                            )
                        })
                        .filter(|&(_, v)| v != 0 && v != u32::MAX)
                        .collect();
                    println!("      ch{ch} nonzero dwords: {nz:?}");
                }
                class2_channels.push((ch, identity, request));
            }
            Err(_) => {}
        }
    }
    let ocp_row = class2_channels.iter().find(|(_, id, _)| *id == 23);
    let zero_row = class2_channels.iter().find(|(_, id, _)| *id == 0);
    println!(
        "    identity-23 row: {:?}; identity-0 row: {:?}",
        ocp_row, zero_row
    );

    // ---- A2. v3|3240 大表条目解析（min/default/max 三元组找 OCP 行） ----
    if let Ok(v3) = gpu.power_readback_table_v3() {
        println!("\n[A2] v3|3240 entry parse (mask@+4, entries@+8, stride 40, 80 slots):");
        let mask = u32::from_le_bytes(v3[4..8].try_into().unwrap());
        println!("    mask = 0x{mask:08X}");
        for ch in 0..80usize {
            if mask != 0 && mask & (1u32 << (ch & 31)) == 0 && ch < 32 {
                continue;
            }
            let base = 8 + 40 * ch;
            if base + 40 > v3.len() {
                break;
            }
            let d: Vec<u32> = (0..10)
                .map(|i| u32::from_le_bytes(v3[base + i * 4..base + i * 4 + 4].try_into().unwrap()))
                .collect();
            if d.iter().all(|&v| v == 0 || v == u32::MAX) {
                continue;
            }
            println!(
                "    v3 ch{ch}: min={}(0x{:X}) def={} max={} | +16..36: {:?}",
                d[0],
                d[0],
                d[1],
                d[2],
                &d[4..]
            );
        }
    }

    // ---- D/E. F8/FE v1|1320 布局对照（raw dump ch0 entry dwords） ----
    println!("\n[D] F8/FE v1|1320 (0x33AB0353/0x17695269) ch0 raw layout:");
    let get_qi = nvapi::sys::nvapi_QueryInterface(0x33AB0353).expect("QI GET 0x33AB0353");
    let mut buf = vec![0u8; 1320];
    buf[..4].copy_from_slice(&0x0001_0528u32.to_ne_bytes());
    buf[4..8].copy_from_slice(&1u32.to_ne_bytes()); // ch0 mask
    type QiFn = unsafe extern "C" fn(
        nvapi::sys::handles::NvPhysicalGpuHandle,
        *mut std::ffi::c_void,
    ) -> u32;
    let get_fn: QiFn = unsafe { std::mem::transmute(get_qi) };
    let status = unsafe { get_fn(*gpu.handle(), buf.as_mut_ptr() as *mut _) };
    println!("    GET status = {status} ({})", status as i32);
    if status == 0 {
        let stamp = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let mask = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        println!("    stamp=0x{stamp:X} mask=0x{mask:X}");
        println!("    entry0 (base +8, 40B) dwords:");
        for i in 0..10 {
            let v = u32::from_le_bytes(buf[8 + i * 4..8 + i * 4 + 4].try_into().unwrap());
            println!("      entry+{:2}: 0x{:08X} ({v})", i * 4, v);
        }
        // mVolt 语义位：值 @40*(ch+1) = entry+32，类型码 @entry+36（248/254）
        let val = u32::from_le_bytes(buf[40..44].try_into().unwrap());
        let cmd = u32::from_le_bytes(buf[44..48].try_into().unwrap());
        println!("    => mVolt 语义位: value(entry+32)={val}, type(entry+36)={cmd} (0x{cmd:X})");
    }
    for cmd in [0xF8u32, 0xFE] {
        match gpu.power_command(0, cmd) {
            Ok(v) => println!("    power_command(ch0, 0x{cmd:X}) = {v}"),
            Err(e) => println!("    power_command(ch0, 0x{cmd:X}) FAIL {}", status_of(&e)),
        }
    }

    // ---- 写臂（默认关）。合法目标 = identity ∈ {0, 23} 行（mVolt kind-27
    // 校验同款约束；identity-6 等其他行驱动 SET 拒 -1，2070 实证）。 ----
    let write_mode = std::env::var("NVOC_POWER_CONTROL_WRITE").ok();
    if let Some(mode) = write_mode {
        println!("\n[WRITE] NVOC_POWER_CONTROL_WRITE={mode} —— 写臂启用");
        let geom = detected.unwrap_or(&POWER_CONTROL_CLASS1);
        let ch: usize = std::env::var("NVOC_POWER_CONTROL_WRITE_CH")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                // 自动选行：identity-0 优先，其次 23（class2 扫描结果）
                match zero_row {
                    Some(&(c, _, _)) => c,
                    None => ocp_row.map(|&(c, _, _)| c).unwrap_or(0),
                }
            });
        let force = std::env::var("NVOC_POWER_CONTROL_FORCE").is_ok();
        let cur = gpu.power_control_table(geom, ch).expect("pre-write GET");
        let base = geom.entry_off + geom.stride * ch;
        let identity = u32::from_le_bytes(cur[base..base + 4].try_into().unwrap());
        if !matches!(identity, 0 | 23) && !force {
            println!(
                "    ABORT: ch{ch} identity={identity} 不在合法集 {{0,23}}（驱动 SET 拒 -1）。\
                 合法通道见 [C]/[C2] 扫描；确要试请加 NVOC_POWER_CONTROL_FORCE=1"
            );
        } else {
            let before = u32::from_le_bytes(cur[base + 68..base + 72].try_into().unwrap());
            let target = if mode == "echo" {
                println!(
                    "    echo 模式：ch{ch}(identity {identity}) entry+68 原值 {before} 回写（零改动验证）"
                );
                before
            } else {
                let v = mode.parse::<u32>().expect("NVOC_POWER_CONTROL_WRITE=<u32>");
                println!("    真写：ch{ch}(identity {identity}) entry+68 {before} → {v}");
                v
            };
            gpu.set_power_control_entry(geom, ch, target).expect("SET");
            // mVolt 12×100ms 轮询，连续 2 次一致判保留
            let mut streak = 0;
            let mut retained = None;
            for _ in 0..12 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let rb = gpu.power_control_table(geom, ch).expect("readback GET");
                let v = u32::from_le_bytes(rb[base + 68..base + 72].try_into().unwrap());
                if v == target {
                    streak += 1;
                    if streak == 2 {
                        retained = Some(v);
                        break;
                    }
                } else {
                    streak = 0;
                }
            }
            match retained {
                Some(v) => println!("    RETAINED: entry+68 = {v}（连续 2 次读回一致）"),
                None => {
                    println!("    NOT RETAINED —— 驱动未保留该写值");
                    if target != before {
                        println!("    还原 {before}…");
                        gpu.set_power_control_entry(geom, ch, before)
                            .expect("restore SET");
                    }
                }
            }
        }
    } else {
        println!("\n[WRITE] 写臂关闭（NVOC_POWER_CONTROL_WRITE 未设置）——纯只读探针");
    }
}

//! PerfVFE 大戳层探针(mVolt+ 0.50.13 实际读写的 tier,2026-11-14)。
//!
//! 背景:小戳 GET(0x10ACC/70344/83996/85016)返回静态配置投影——boost/slot2
//! 扰动下 var/equ control 零跟动。mVolt+ sub_14028B700 的五个 blob 全部用大
//! 戳:VarGetControl 171976(0x2A008,40904B,含 32B 身份@+4)、VarGetInfo
//! 3118440(0x2F9568,2856296B)、EquGetInfo 885828(0xD8444,623684B)、
//! EquGetControl 1410116(0x1584C4,+1024B 身份@+4)。本探针验证大戳在 2070
//! 上的接受度与内容,并做 slot2 扰动 diff 定位消耗面变量。
//!
//! Run: cargo test --test vfe_v2_tier_probe -- --ignored --nocapture
//! (slot2 扰动臂需 NVOC_ALLOW_VFE_PERTURB=1 + 提权)

#![allow(unused_must_use)]

use core::ptr;
use nvapi::PhysicalGpu;
use nvapi::sys::api::{
    NvAPI_GPU_PerfVfeEquGetControl, NvAPI_GPU_PerfVfeEquGetInfo, NvAPI_GPU_PerfVfeVarGetControl,
    NvAPI_GPU_PerfVfeVarGetInfo,
};

type VfeFfi = unsafe fn(
    nvapi::sys::handles::NvPhysicalGpuHandle,
    *mut core::ffi::c_void,
) -> nvapi::sys::NvAPI_Status;

fn get_raw(
    ffi: VfeFfi,
    handle: nvapi::sys::handles::NvPhysicalGpuHandle,
    stamp: u32,
    size: usize,
    identity: &[u8],
) -> (i32, Vec<u8>) {
    unsafe { get_raw_impl(ffi, handle, stamp, size, identity) }
}

unsafe fn get_raw_impl(
    ffi: VfeFfi,
    handle: nvapi::sys::handles::NvPhysicalGpuHandle,
    stamp: u32,
    size: usize,
    identity: &[u8],
) -> (i32, Vec<u8>) {
    let mut buf = vec![0u8; size];
    buf[0..4].copy_from_slice(&stamp.to_le_bytes());
    let id_len = identity.len().min(size - 4);
    buf[4..4 + id_len].copy_from_slice(&identity[..id_len]);
    let st = ffi(handle, buf.as_mut_ptr().cast());
    (st as i32, buf)
}

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

fn f32_of(v: u32) -> f32 {
    f32::from_le_bytes(v.to_le_bytes())
}

#[test]
#[ignore]
fn vfe_v2_tier_probe() {
    nvapi::initialize().expect("init");
    let mut gpus = PhysicalGpu::enumerate().expect("enumerate");
    let gpu = gpus.drain(..).next().expect("no gpu");
    let handle = *gpu.handle();
    println!("GPU: {:?}", gpu.full_name());

    // identity 32B:先零身份试(驱动可能在响应中回填)
    let zero32 = [0u8; 32];
    let zero1024 = [0u8; 1024];

    println!("=== Var GetControl V2 (0x29FC8, 40904B, 32B identity@+4) ===");
    let (st, buf) = get_raw(
        |h, p| unsafe { NvAPI_GPU_PerfVfeVarGetControl(h, p.cast()) },
        handle,
        171976,
        40904,
        &zero32,
    );
    println!("status={st} ({st:#x})");
    let nz = nonzero_dwords(&buf, 24);
    println!("nonzero dwords (first 24): {nz:?}");
    for (off, v) in &nz {
        if (20..40904 - 4).contains(off) {
            println!("  +{off:04X}: {v:#010x} (float {})", f32_of(*v));
        }
    }

    println!("=== Var GetInfo V2 (0x2F9568, 2856296B) ===");
    let (st_i, buf_i) = get_raw(
        |h, p| unsafe { NvAPI_GPU_PerfVfeVarGetInfo(h, p.cast()) },
        handle,
        3118440,
        2856296,
        &[],
    );
    println!("status={st_i} ({st_i:#x})");
    let nz_i = nonzero_dwords(&buf_i, 16);
    println!("nonzero dwords (first 16): {nz_i:?}");

    println!("=== Equ GetInfo 0xD8444 (623684B) ===");
    let (st_e, _buf_e) = get_raw(
        |h, p| unsafe { NvAPI_GPU_PerfVfeEquGetInfo(h, p.cast()) },
        handle,
        885828,
        623684,
        &[],
    );
    println!("status={st_e} ({st_e:#x})");

    println!("=== Equ GetControl 0x158444 (1410116B, 1024B identity@+4) ===");
    let (st_c, buf_c) = get_raw(
        |h, p| unsafe { NvAPI_GPU_PerfVfeEquGetControl(h, p.cast()) },
        handle,
        1410116,
        1410116,
        &zero1024,
    );
    println!("status={st_c} ({st_c:#x})");
    let nz_c = nonzero_dwords(&buf_c, 12);
    println!("nonzero dwords (first 12): {nz_c:?}");

    // 掩码发现:engine+184 = 256 位 var 选择掩码(非身份!mVolt+ SET 组包
    // sub_14028C6F0: byte(4 + idx/8) |= 1<<(idx%8))。GET 时掩码选要读的 var。
    let mask_all = {
        let mut m = vec![0u8; 32];
        for b in m.iter_mut().take(25) {
            *b = 0xFF;
        }
        m
    };
    println!("=== Var GetControl V2 with FULL mask (bits 0..199) ===");
    let (st_m, buf_m) = get_raw(
        |h, p| unsafe { NvAPI_GPU_PerfVfeVarGetControl(h, p.cast()) },
        handle,
        171976,
        40904,
        &mask_all,
    );
    println!("status={st_m}");
    let nz_m = nonzero_dwords(&buf_m, 30);
    println!("nonzero dwords (first 30): {nz_m:?}");
    for (off, v) in nz_m.iter().skip(1).take(12) {
        println!("  +{off:04X}: {v:#010x} (float {})", f32_of(*v));
    }
    let richness_m = nonzero_dwords(&buf_m, usize::MAX).len();
    println!("total nonzero dwords: {richness_m}");

    // slot2 扰动臂:var V2 是否跟动(melonVolt 车道 → VFE 大层耦合检验)
    if std::env::var("NVOC_ALLOW_VFE_PERTURB").is_ok() && st == 0 {
        println!("=== slot2 perturb diff on Var V2 ===");
        let base = buf.clone();
        let step = gpu
            .volt_devices()
            .ok()
            .and_then(|d| d.first().map(|d| d.step_uV))
            .filter(|s| *s > 0)
            .unwrap_or(6250) as i32;
        let cur = gpu
            .volt_rails()
            .expect("rails")
            .control
            .first()
            .unwrap()
            .values[2];
        gpu.set_volt_rail_slot(0, 2, cur.wrapping_add(step))
            .expect("slot2 write");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (_, after) = get_raw(
            |h, p| unsafe { NvAPI_GPU_PerfVfeVarGetControl(h, p.cast()) },
            handle,
            171976,
            40904,
            &zero32,
        );
        let mut diffs = 0usize;
        for (i, (a, b)) in base.chunks_exact(4).zip(after.chunks_exact(4)).enumerate() {
            if a != b {
                let va = u32::from_le_bytes(a.try_into().unwrap());
                let vb = u32::from_le_bytes(b.try_into().unwrap());
                println!(
                    "  CHANGED +{:04X}: {va:#010x} -> {vb:#010x} (float {} -> {})",
                    i * 4,
                    f32_of(va),
                    f32_of(vb)
                );
                diffs += 1;
                if diffs >= 20 {
                    break;
                }
            }
        }
        if diffs == 0 {
            println!("  no Var V2 dwords moved under slot2 perturbation");
        }
        // 还原
        gpu.set_volt_rail_slot(0, 2, cur).ok();
        std::thread::sleep(std::time::Duration::from_millis(300));
        println!(
            "restored, status {:?}",
            gpu.volt_rails().map(|r| r.status.first().map(|s| s.values))
        );
    } else if st != 0 {
        println!("Var V2 GET refused — write arm skipped");
    } else {
        println!("write arm skipped — set NVOC_ALLOW_VFE_PERTURB=1");
    }
    // V2 目录扫描:255 var 逐个单 bit GET(type/min/max/applied/value),
    // 建工厂变量全目录。GET-only,无写。
    {
        println!("=== Var V2 factory directory scan (255 vars) ===");
        use nvapi::sys::api::NvAPI_GPU_PerfVfeVarGetControl;
        let mut rows = Vec::new();
        for idx in 0..255usize {
            let mut req = vec![0u8; 40904];
            req[0..4].copy_from_slice(&171976u32.to_le_bytes());
            req[4 + idx / 8] = 1 << (idx % 8);
            let st =
                unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) } as i32;
            if st != 0 {
                continue;
            }
            let rec = 104 + 160 * idx;
            let t = u32::from_le_bytes(req[rec..rec + 4].try_into().unwrap());
            if t == 0 {
                continue;
            }
            let mn = f32::from_le_bytes(req[rec + 4..rec + 8].try_into().unwrap());
            let mx = f32::from_le_bytes(req[rec + 8..rec + 12].try_into().unwrap());
            let ap = req[rec + 44];
            let vl = f32::from_le_bytes(req[rec + 48..rec + 52].try_into().unwrap());
            rows.push((idx, t, mn, mx, ap, vl));
        }
        println!("populated vars: {}/255", rows.len());
        for (idx, t, mn, mx, ap, vl) in &rows {
            println!("  var{idx}: type={t} min={mn} max={mx} applied={ap} value={vl}");
        }
    }

    // V2 写臂 v4(§13.3 设计):**var0(NVVDD REL offset)单变量单会话**。
    // 门 = NVOC_ALLOW_VFE_VAR0_WRITE(与 DANGER sweep 的 NVOC_ALLOW_VFE_WRITE
    // 分离——sweep 是死锁事故臂,绝不共门)。值 = NVOC_VFE_VAR0_VALUE_UV
    // (默认 20000 = +20mV,stock ceiling 语义对齐)。工厂记录原样作 base
    // (50 系 type=13 天然 SET-ready),只 patch min/max/applied/value → SET
    // → 读回+墙值三查(before/mid/after)→ 工厂记录 SET 回还原 → 复核。
    if std::env::var("NVOC_ALLOW_VFE_VAR0_WRITE").is_ok() {
        let idx: usize = std::env::var("NVOC_VFE_VAR_INDEX")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        println!("=== [WRITE] var{idx} single-var SET + auto-restore (gate NVOC_ALLOW_VFE_VAR0_WRITE) ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeVarGetControl, NvAPI_GPU_PerfVfeVarSetControl};
        let value: f32 = std::env::var("NVOC_VFE_VAR0_VALUE_UV")
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(20000.0);
        println!("  target value = {value} uV (offset)");
        let walls = |tag: &str| {
            if let Ok(r) = gpu.volt_rails() {
                if let Some(s) = r.status.first() {
                    // [current, target-wall, vbios-wall, effective, vrm-max, min-hold]
                    println!("  [{tag}] rail0 status uV: {:?}", s.values);
                }
            }
        };
        let single_get = |idx: usize| -> Vec<u8> {
            let mut req = vec![0u8; 40904];
            req[0..4].copy_from_slice(&171976u32.to_le_bytes());
            req[4 + idx / 8] = 1 << (idx % 8);
            unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) };
            req
        };
        walls("before");
        let factory = single_get(idx);
        let rec = 104 + 160 * idx;
        let f_type = u32::from_le_bytes(factory[rec..rec + 4].try_into().unwrap());
        println!(
            "factory var{idx}@{rec}: type={f_type} min={} max={} applied={} value={}",
            f32::from_le_bytes(factory[rec + 4..rec + 8].try_into().unwrap()),
            f32::from_le_bytes(factory[rec + 8..rec + 12].try_into().unwrap()),
            factory[rec + 44],
            f32::from_le_bytes(factory[rec + 48..rec + 52].try_into().unwrap()),
        );

        // patch:单 bit 掩码 + min/max/applied/value,其余字节原样
        let mut req = factory.clone();
        for x in req[4..36].iter_mut() {
            *x = 0;
        }
        req[4 + idx / 8] = 1 << (idx % 8);
        req[rec + 4..rec + 8].copy_from_slice(&value.min(0.0).to_le_bytes());
        req[rec + 8..rec + 12].copy_from_slice(&value.max(0.0).to_le_bytes());
        req[rec + 44] = 1;
        req[rec + 48..rec + 52].copy_from_slice(&value.to_le_bytes());
        let st_set = unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, req.as_ptr().cast()) } as i32;
        std::thread::sleep(std::time::Duration::from_millis(300));
        let back = single_get(idx);
        let rd_ap = back[rec + 44];
        let rd_v = f32::from_le_bytes(back[rec + 48..rec + 52].try_into().unwrap());
        println!(
            "  SET={st_set} readback: applied={rd_ap} value={rd_v} {}",
            if rd_ap == 1 && rd_v == value {
                "<— RETAINED"
            } else {
                "(dropped/mismatch)"
            }
        );
        walls("mid");

        // 还原:工厂记录原样 SET 回(applied=0/value=0)
        unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, factory.as_ptr().cast()) };
        std::thread::sleep(std::time::Duration::from_millis(300));
        let rst = single_get(idx);
        let rs_ap = rst[rec + 44];
        let rs_v = f32::from_le_bytes(rst[rec + 48..rec + 52].try_into().unwrap());
        println!(
            "  restore readback: applied={rs_ap} value={rs_v} {}",
            if rs_ap == 0 {
                "<— restored"
            } else {
                "<— RESTORE FAILED, rerun with value=0 or reboot"
            }
        );
        walls("after");
    }

    // ⚠️⚠️⚠️ 危险——本臂已实机造成内核死锁(2026-11-14,黑屏卡死无 TDR):
    // 19 变量连续 applied=1 微扰 → 驱动真实消费(SET 保留已实证) → VFE 策略
    // 引擎死锁,死锁级别低于 TDR 看门狗,只能重启恢复。**禁止再运行**。
    // 语义映射必须走 RE 路线(Equ 表结构分析/mVolt 流复刻),禁止盲扰。
    // 语义映射扫描(NVOC_ALLOW_VFE_WRITE=1):19 个 var 逐个 +6250µV 微扰 →
    // volt_rails 六值 diff → 还原 → 复核。产出 var→墙 语义映射。
    if std::env::var("NVOC_ALLOW_VFE_WRITE").is_ok() {
        println!("=== Var semantics sweep (19 vars, +6250uV perturbation) ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeVarGetControl, NvAPI_GPU_PerfVfeVarSetControl};
        let rails_now = |tag: &str| -> Option<[i32; 6]> {
            let r = gpu.volt_rails().ok()?;
            let s = r.status.first()?.values;
            println!("  [{tag}] status: {s:?}");
            Some(s)
        };
        let before = rails_now("baseline");
        let populated: Vec<usize> = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
        ]
        .to_vec();
        for &idx in &populated {
            let single_get = |i: usize| -> Vec<u8> {
                let mut req = vec![0u8; 40904];
                req[0..4].copy_from_slice(&171976u32.to_le_bytes());
                req[4 + i / 8] = 1 << (i % 8);
                unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) };
                req
            };
            let factory = single_get(idx);
            let rec = 104 + 160 * idx;
            // 工厂值读取(若 value/min/max 已有值,微扰叠加)
            let f_val = f32::from_le_bytes(factory[rec + 48..rec + 52].try_into().unwrap());
            let perturb = if f_val == 0.0 {
                6250.0f32
            } else {
                f_val + 6250.0
            };
            let mut req = factory.clone();
            for x in req[4..36].iter_mut() {
                *x = 0;
            }
            req[4 + idx / 8] = 1 << (idx % 8);
            req[rec + 4..rec + 8].copy_from_slice(&perturb.min(0.0).to_le_bytes());
            req[rec + 8..rec + 12].copy_from_slice(&perturb.max(0.0).to_le_bytes());
            req[rec + 44] = 1;
            req[rec + 48..rec + 52].copy_from_slice(&perturb.to_le_bytes());
            let st_set =
                unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, req.as_ptr().cast()) } as i32;
            std::thread::sleep(std::time::Duration::from_millis(250));
            let after_w = rails_now(&format!("var{idx}=+{perturb}"));
            // 还原
            unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, factory.as_ptr().cast()) };
            std::thread::sleep(std::time::Duration::from_millis(250));
            let _ = st_set;
            if let (Some(b), Some(a)) = (before, after_w) {
                let moved: Vec<String> = b
                    .iter()
                    .zip(a.iter())
                    .enumerate()
                    .filter(|(_, (x, y))| x != y)
                    .map(|(i, (x, y))| format!("s{i}:{x}->{y}"))
                    .collect();
                if !moved.is_empty() {
                    println!("    *** var{idx} MOVED: {} ***", moved.join(" "));
                }
            }
        }
        rails_now("final");
    }

    // 身份自举实验:小戳 EquGetControl(85016)raw 响应中找非零区块 →
    // 候选 1024B/32B 切片回喂 V2 GET → 量丰富度。
    {
        println!("=== identity bootstrap from small-tier EquGetControl ===");
        use nvapi::sys::api::NvAPI_GPU_PerfVfeEquGetControl;
        let mut buf = vec![0u8; 85016];
        buf[0..4].copy_from_slice(&85016u32.to_le_bytes());
        // 掩码播种:前 64 位
        for d in 0..8usize {
            buf[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        let st_s =
            unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, buf.as_mut_ptr().cast()) } as i32;
        println!("small-tier EquGetControl status={st_s}");
        let nz = nonzero_dwords(&buf, usize::MAX);
        println!("total nonzero dwords: {}", nz.len());
        // 找连续非零区块(可能的身份/数据区)
        let mut regions: Vec<(usize, usize)> = Vec::new();
        let mut cur: Option<usize> = None;
        for (off, _) in &nz {
            match cur {
                None => cur = Some(*off),
                Some(start) => {
                    if *off - (start + 4) > 64 {
                        regions.push((start, *off));
                        cur = Some(*off);
                    }
                }
            }
        }
        if let Some(start) = cur {
            regions.push((start, nz.last().unwrap().0));
        }
        println!(
            "nonzero regions ({}): {:?}…",
            regions.len(),
            &regions[..regions.len().min(10)]
        );
        // 候选:每个区块起点对齐后的 1024B 切片 → Var V2 GET 丰富度
        for (start, _) in regions.iter().take(6) {
            let base = start - start % 4;
            if base + 1024 > buf.len() {
                continue;
            }
            let ident = buf[base..base + 1024].to_vec();
            let (_, probe_v2) = get_raw(
                |h, p| unsafe { NvAPI_GPU_PerfVfeVarGetControl(h, p.cast()) },
                handle,
                171976,
                40904,
                &ident[..32.min(ident.len())],
            );
            let r32 = nonzero_dwords(&probe_v2, usize::MAX).len();
            let (_, probe_e) = get_raw(
                |h, p| unsafe { NvAPI_GPU_PerfVfeEquGetControl(h, p.cast()) },
                handle,
                1410116,
                1410116,
                &ident,
            );
            let r1024 = nonzero_dwords(&probe_e, usize::MAX).len();
            println!(
                "  ident@+{base:04X} (nz={}): VarV2 richness={r32}, EquV2 richness={r1024}",
                ident.iter().filter(|&b| *b != 0).count()
            );
            if r1024 > 20 || r32 > 10 {
                println!("    *** RICH — identity candidate found ***");
            }
        }
    }
    // 请求形状扫描:Var V2 GET 的 身份/掩码 摆放位置穷举(丰富度判据)。
    // 候选形状:S1 mask32@4(基线) / S2 ident1024@4 / S3 mask32@4+ident1024@36 /
    // S4 ident1024@36+mask32@4 / S5 ident32@4+ident992@36。
    {
        println!("=== Var V2 request-shape sweep (richness criterion) ===");
        // ident1024 = 小戳 EquGetControl 响应 @0..1024(已证实可过 Equ V2 门)
        use nvapi::sys::api::NvAPI_GPU_PerfVfeEquGetControl;
        let mut small = vec![0u8; 85016];
        small[0..4].copy_from_slice(&85016u32.to_le_bytes());
        for d in 0..8usize {
            small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        let st_s =
            unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) } as i32;
        println!("small-tier GET status={st_s}");
        let ident1024: Vec<u8> = small[0..1024].to_vec();
        let ident32: Vec<u8> = ident1024[..32].to_vec();

        let mut mk = |stamp: u32, size: usize, layout: &dyn Fn(&mut [u8])| -> (i32, usize) {
            let mut b = vec![0u8; size];
            b[0..4].copy_from_slice(&stamp.to_le_bytes());
            layout(&mut b);
            let st =
                unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, b.as_mut_ptr().cast()) } as i32;
            let r = nonzero_dwords(&b, usize::MAX).len();
            (st, r)
        };

        let shapes: Vec<(&str, Box<dyn Fn(&mut [u8])>)> = vec![
            (
                "S1 mask32@4",
                Box::new(|b: &mut [u8]| {
                    for i in 0..25 {
                        b[4 + i] = 0xFF;
                    }
                }),
            ),
            (
                "S2 ident1024@4",
                Box::new(|b: &mut [u8]| {
                    b[4..1028].copy_from_slice(&ident1024);
                }),
            ),
            (
                "S3 mask32@4+ident1024@36",
                Box::new(|b: &mut [u8]| {
                    for i in 0..25 {
                        b[4 + i] = 0xFF;
                    }
                    b[36..1060].copy_from_slice(&ident1024);
                }),
            ),
            (
                "S4 ident1024@36+mask32@4",
                Box::new(|b: &mut [u8]| {
                    for i in 0..25 {
                        b[4 + i] = 0xFF;
                    }
                    b[36..1060].copy_from_slice(&ident1024);
                }),
            ),
            (
                "S5 ident32@4+ident992@36",
                Box::new(|b: &mut [u8]| {
                    b[4..36].copy_from_slice(&ident32);
                    b[36..1028].copy_from_slice(&ident1024[32..1024]);
                }),
            ),
            (
                "S6 ident1024@4+mask32@1028",
                Box::new(|b: &mut [u8]| {
                    b[4..1028].copy_from_slice(&ident1024);
                    for i in 0..25 {
                        b[1028 + i] = 0xFF;
                    }
                }),
            ),
        ];
        for (label, layout) in &shapes {
            let (st, r) = mk(171976, 40904, layout.as_ref());
            println!("  {label}: status={st}, richness={r}");
            if r > 12 {
                println!("    *** RICH — Var V2 GET shape found ***");
            }
        }
        // S3 形状完整非零分布 + float 解读(墙值对齐:1125/1093.75/1068.75/712.5)
        {
            let mut b = vec![0u8; 40904];
            b[0..4].copy_from_slice(&171976u32.to_le_bytes());
            for i in 0..25 {
                b[4 + i] = 0xFF;
            }
            b[36..1060].copy_from_slice(&ident1024);
            let st =
                unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, b.as_mut_ptr().cast()) } as i32;
            println!("S3 full dump: status={st}");
            let nz = nonzero_dwords(&b, usize::MAX);
            println!("total nonzero: {}", nz.len());
            let walls = [
                1125.0f32, 1093.75, 1068.75, 712.5, 1125000.0, 1093750.0, 1068750.0, 712500.0,
                130.0, 60.0, 80.0, 32.0,
            ];
            for (off, v) in nz.iter().take(40) {
                let f = f32::from_le_bytes(v.to_le_bytes());
                let wall_tag = if walls.iter().any(|w| (w - f).abs() < 0.01) {
                    " <— WALL MATCH"
                } else {
                    ""
                };
                println!("  +{off:04X}: {v:#010x} (f32 {f}{wall_tag})");
            }
            // 全表墙值搜索(不限前 40)
            let mut hits = 0usize;
            for (off, v) in &nz {
                let f = f32::from_le_bytes(v.to_le_bytes());
                if walls.iter().any(|w| (w - f).abs() < 0.01) {
                    println!("  WALL@+{off:04X} = {f}");
                    hits += 1;
                }
            }
            println!("wall-value hits: {hits}");
        }

        // 对照:Equ V2 用 S2 形状(身份@4)应保持 rich
        let mut eb = vec![0u8; 1410116];
        eb[0..4].copy_from_slice(&1410116u32.to_le_bytes());
        eb[4..1028].copy_from_slice(&ident1024);
        let st_e = unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, eb.as_mut_ptr().cast()) } as i32;
        println!(
            "Equ V2 ident@4 control: status={st_e}, richness={}",
            nonzero_dwords(&eb, usize::MAX).len()
        );
    }
    // VarGetInfo 大层(0x2F9568)掩码版 GET(零身份,+4=掩码):19 记录逐 dword
    // dump + 字符串 hunt。早前实测:零身份返回 mask 0x7FFFF + 记录 @296 stride 276。
    {
        println!("=== Var GetInfo V2 mask-only GET — 19-record dword dump ===");
        use nvapi::sys::api::NvAPI_GPU_PerfVfeVarGetInfo;
        let size = 2856296usize;
        let mut b = vec![0u8; size];
        b[0..4].copy_from_slice(&3118440u32.to_le_bytes());
        // 掩码:19 位(与工厂目录一致)
        b[4] = 0xFF;
        b[5] = 0xFF;
        b[6] = 0x07;
        let st = unsafe { NvAPI_GPU_PerfVfeVarGetInfo(handle, b.as_mut_ptr().cast()) } as i32;
        println!("GET status={st}");
        // 找记录基:扫描前 400 字节的非零分布
        let mut nz_head = Vec::new();
        for (i, w) in b[..4096].chunks_exact(4).enumerate() {
            let v = u32::from_le_bytes(w.try_into().unwrap());
            if v != 0 {
                nz_head.push((i * 4, v));
            }
        }
        println!("head nonzero (first 4KB): {nz_head:?}");
        // 记录基尝试 +296 stride 276(早前实测)
        for entry in 0..19usize {
            let base = 296 + 276 * entry;
            if base + 276 > size {
                break;
            }
            let mut fields = Vec::new();
            for (i, w) in b[base..base + 276].chunks_exact(4).enumerate() {
                let v = u32::from_le_bytes(w.try_into().unwrap());
                if v != 0 {
                    let f = f32::from_le_bytes(v.to_le_bytes());
                    fields.push(format!("+{:03X}:{v:#x}(f32 {f:.3?})", i * 4));
                }
            }
            if !fields.is_empty() {
                println!("  entry{entry}@{base:X}: {}", fields.join(" "));
            }
        }
        // ASCII 串 hunt(整缓冲)
        let mut found = 0usize;
        let mut i2 = 4usize;
        while i2 + 1 < size && found < 30 {
            let mut chars = Vec::new();
            let mut j = i2;
            while j < size {
                let c = b[j];
                if c.is_ascii_alphanumeric() || c == b'_' || c == b'/' || c == b'[' || c == b']' {
                    chars.push(c);
                    j += 1;
                } else {
                    break;
                }
            }
            if chars.len() >= 5 {
                println!("  ascii@+{i2:06X}: {}", String::from_utf8_lossy(&chars));
                found += 1;
                i2 = j + 1;
            } else {
                i2 += 1;
            }
        }
        println!("ascii strings: {found}");
    }
    // ============ 第七轮(2026-11-15,5060 Ti 目录判决已回)============
    // 5060 Ti 实测:76/255 populated;var45(type9,450000..1250000µV)=电压域候选
    // (2070 var13 同角色,索引漂移实锤);var14=频率类;var15/16=百分比类。
    // 采样时活体墙(melonVolt offset +200mV 已消费):VBIOS 1220mV/VRM 1200/
    // ceiling 200mV/min-hold 625mV/当前 1145mV;stock 墙=1020/1200/635/20mV。
    // ⇒ S3 记录里的 20000(=stock ceiling 20mV)提示 76B 记录携带工厂快照值。
    // 本轮全 GET-only:①info 全条目(RM name ID) ②160B 工厂记录全 dump
    // ③单 bit+身份 record↔index 钉死 ④Equ 层墙值定位 ⑤身份块授权域对比。
    let wall_set: &[(&str, f32)] = &[
        ("VBIOS-wall-now-1220mV", 1220000.0),
        ("OV-VRM-wall-1200mV", 1200000.0),
        ("current-1145mV", 1145000.0),
        ("minhold-now-625mV", 625000.0),
        ("ceiling-now-200mV", 200000.0),
        ("VBIOS-wall-stock-1020mV", 1020000.0),
        ("stock-wall-1000mV", 1000000.0),
        ("minhold-stock-635mV", 635000.0),
        ("stock-ceiling-20mV", 20000.0),
        ("VRM-min-400mV", 400000.0),
        ("VRM-max-1280mV", 1280000.0),
        ("VRM-default-900mV", 900000.0),
        ("descA-800mV", 800000.0),
        ("descB-700mV", 700000.0),
        ("obs-810mV", 810000.0),
        ("obs-1100mV", 1100000.0),
        ("var45max-1250mV", 1250000.0),
        ("var45min-450mV", 450000.0),
    ];
    let wall_tag = |f: f32| -> String {
        wall_set
            .iter()
            .find(|item| (item.1 - f).abs() < 0.5)
            .map(|item| format!(" <{}>", item.0))
            .unwrap_or_default()
    };

    // [H1] VarGetInfo 全掩码(256 位)全条目 dump —— RM name ID 收集
    {
        println!("=== [H1] Var GetInfo V2 FULL-mask (256 bits) all-entry dump ===");
        use nvapi::sys::api::NvAPI_GPU_PerfVfeVarGetInfo;
        let size = 2856296usize;
        let mut b = vec![0u8; size];
        b[0..4].copy_from_slice(&3118440u32.to_le_bytes());
        for i in 0..32 {
            b[4 + i] = 0xFF;
        }
        let st = unsafe { NvAPI_GPU_PerfVfeVarGetInfo(handle, b.as_mut_ptr().cast()) } as i32;
        println!("GET status={st}");
        let mut nz_head = Vec::new();
        for (i, w) in b[..4096].chunks_exact(4).enumerate() {
            let v = u32::from_le_bytes(w.try_into().unwrap());
            if v != 0 {
                nz_head.push((i * 4, v));
            }
        }
        println!("head nonzero (first 4KB): {nz_head:?}");
        let mut populated = 0usize;
        for k in 0..300usize {
            let base = 296 + 276 * k;
            if base + 276 > size {
                break;
            }
            let t = u32::from_le_bytes(b[base..base + 4].try_into().unwrap());
            if t == 0 {
                continue;
            }
            populated += 1;
            let mut fields = Vec::new();
            for (i, w) in b[base..base + 276].chunks_exact(4).enumerate() {
                let v = u32::from_le_bytes(w.try_into().unwrap());
                if v != 0 {
                    fields.push(format!("+{:03X}:{v:#010x}(lo16 {:#06x})", i * 4, v & 0xFFFF));
                }
            }
            println!("  entry{k}@{base:X}: type={t} {}", fields.join(" "));
        }
        println!("populated info entries: {populated}");
    }

    // [H2+H3] populated var 全记录:160B 工厂记录(round-4 几何)+ 单 bit+身份
    // S3 形态 GET(record↔index 钉死;S3 记录区 @136 步进 76)
    {
        println!("=== [H2+H3] per-var factory record + identity GET (record<->index pin) ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeEquGetControl, NvAPI_GPU_PerfVfeVarGetControl};
        let mut small = vec![0u8; 85016];
        small[0..4].copy_from_slice(&85016u32.to_le_bytes());
        for d in 0..8usize {
            small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) };
        let ident1024: Vec<u8> = small[0..1024].to_vec();

        let mut populated = Vec::new();
        for idx in 0..255usize {
            let mut req = vec![0u8; 40904];
            req[0..4].copy_from_slice(&171976u32.to_le_bytes());
            req[4 + idx / 8] = 1 << (idx % 8);
            let st =
                unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) } as i32;
            if st != 0 {
                continue;
            }
            let rec = 104 + 160 * idx;
            let t = u32::from_le_bytes(req[rec..rec + 4].try_into().unwrap());
            if t == 0 {
                continue;
            }
            populated.push(idx);
            let mut fields = Vec::new();
            for (i, w) in req[rec..rec + 160].chunks_exact(4).enumerate() {
                let v = u32::from_le_bytes(w.try_into().unwrap());
                if v != 0 {
                    let f = f32::from_le_bytes(v.to_le_bytes());
                    fields.push(format!("+{:02X}:{v:#010x}(f32 {f:.3?}){}", i * 4, wall_tag(f)));
                }
            }
            println!("  var{idx} factory@{rec}: {}", fields.join(" "));
        }
        println!("populated: {} vars — {:?}", populated.len(), populated);

        println!("  --- single-bit + identity (S3 shape) ---");
        for idx in &populated {
            let mut req = vec![0u8; 40904];
            req[0..4].copy_from_slice(&171976u32.to_le_bytes());
            req[4 + idx / 8] = 1 << (idx % 8);
            req[36..1060].copy_from_slice(&ident1024);
            let st =
                unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) } as i32;
            let nz = nonzero_dwords(&req, usize::MAX);
            let body: Vec<String> = nz
                .iter()
                .filter(|(off, _)| *off >= 36)
                .take(14)
                .map(|(off, v)| {
                    let f = f32::from_le_bytes(v.to_le_bytes());
                    format!("+{off:04X}:{v:#010x}(f32 {f:.3?}){}", wall_tag(f))
                })
                .collect();
            println!("  var{idx}: st={st} nz={} {}", nz.len(), body.join(" "));
        }
    }

    // [H4] Equ V2 control(1410116,身份@4)rich dump + 全缓冲墙值搜索
    // (µV 全套 + mV 标度 400..1280;V/F 方程常数若含墙值即定位消费方程)
    {
        println!("=== [H4] Equ V2 control rich dump + wall-value search ===");
        use nvapi::sys::api::NvAPI_GPU_PerfVfeEquGetControl;
        let mut small = vec![0u8; 85016];
        small[0..4].copy_from_slice(&85016u32.to_le_bytes());
        for d in 0..8usize {
            small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) };
        let ident1024: Vec<u8> = small[0..1024].to_vec();
        let mut eb = vec![0u8; 1410116];
        eb[0..4].copy_from_slice(&1410116u32.to_le_bytes());
        eb[4..1028].copy_from_slice(&ident1024);
        let st = unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, eb.as_mut_ptr().cast()) } as i32;
        let nz = nonzero_dwords(&eb, usize::MAX);
        println!("status={st} nonzero dwords={}", nz.len());
        let mut regions: Vec<(usize, usize)> = Vec::new();
        for (off, _) in &nz {
            match regions.last_mut() {
                Some(last) if *off <= last.1 + 64 => last.1 = *off + 4,
                _ => regions.push((*off, *off + 4)),
            }
        }
        println!(
            "regions ({}): {:?}",
            regions.len(),
            &regions[..regions.len().min(24)]
        );
        let mv_set: &[(&str, f32)] = &[
            ("1220mV", 1220.0),
            ("1200mV", 1200.0),
            ("1145mV", 1145.0),
            ("1100mV", 1100.0),
            ("1020mV", 1020.0),
            ("1000mV", 1000.0),
            ("900mV", 900.0),
            ("810mV", 810.0),
            ("800mV", 800.0),
            ("700mV", 700.0),
            ("635mV", 635.0),
            ("625mV", 625.0),
            ("450mV", 450.0),
            ("400mV", 400.0),
            ("1280mV", 1280.0),
            ("1250mV", 1250.0),
        ];
        let mut hits = 0usize;
        for (off, v) in &nz {
            let f = f32::from_le_bytes(v.to_le_bytes());
            let uv_tag = wall_set
                .iter()
                .find(|item| (item.1 - f).abs() < 0.5)
                .map(|item| format!("µV {}", item.0));
            let mv_tag = mv_set
                .iter()
                .find(|item| (item.1 - f).abs() < 0.01)
                .map(|item| format!("mV {}", item.0));
            if uv_tag.is_some() || mv_tag.is_some() {
                println!("  HIT @+{off:06X}: {f} ({})", uv_tag.or(mv_tag).unwrap());
                hits += 1;
                if hits >= 60 {
                    break;
                }
            }
        }
        println!("wall hits: {hits}");
    }

    // [H5] 6 个 rich 身份块逐个 S3 GET(全掩码)—— 每身份授权的记录集对比
    {
        println!("=== [H5] per-identity-block S3 GET (full mask) record dump ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeEquGetControl, NvAPI_GPU_PerfVfeVarGetControl};
        let mut small = vec![0u8; 85016];
        small[0..4].copy_from_slice(&85016u32.to_le_bytes());
        for d in 0..8usize {
            small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) };
        for base in [0usize, 100, 176, 252, 328, 404] {
            let end = (base + 1024).min(small.len());
            let ident = small[base..end].to_vec();
            let mut req = vec![0u8; 40904];
            req[0..4].copy_from_slice(&171976u32.to_le_bytes());
            for i in 0..25 {
                req[4 + i] = 0xFF;
            }
            let n = ident.len().min(1024);
            req[36..36 + n].copy_from_slice(&ident[..n]);
            let st =
                unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) } as i32;
            let mut recs = Vec::new();
            for k in 0..40usize {
                let r = 136 + 76 * k;
                if r + 76 > req.len() {
                    break;
                }
                let t = u32::from_le_bytes(req[r..r + 4].try_into().unwrap());
                let mn = f32::from_le_bytes(req[r + 4..r + 8].try_into().unwrap());
                let mx = f32::from_le_bytes(req[r + 8..r + 12].try_into().unwrap());
                let vl = f32::from_le_bytes(req[r + 44..r + 48].try_into().unwrap());
                if t != 0 || mn != 0.0 || mx != 0.0 || vl != 0.0 {
                    recs.push(format!("r{k}[t={t} {mn}..{mx} v={vl}]"));
                }
            }
            let richness = nonzero_dwords(&req, usize::MAX).len();
            println!("  ident@{base}: st={st} nz={richness} {}", recs.join(" "));
        }
    }
    // ============ 第八轮(2026-11-15,第七轮判决后)============
    // ⑦判决:round-4 几何跨代际成立(var0 单bit+身份 @104 type13/max127);
    //   引擎活表 Var@136 / Equ@104,76B 步进 [type,min,max,value@44],
    //   值集 {810000,900000,0,900..1100000,20000,700000,1200000,+r7/r9-r12};
    //   墙值(1220/1020/625/635)不在 VFE 层;身份块 @100/176/252/328/404 的
    //   响应内容位置未知;info 全掩码只回 2 条(掩码语义不同)。
    // mVolt 模型:var_slot=4*rail+map[3,0,1,2] → NVVDD REL=var0(0..127 代际
    //   稳定界,2070 SET var0=6250µV 影响活体电压实证)→ 5060 Ti 抬墙候选=var0。
    // 本轮全 GET-only:①驱动版本 ②引擎表全字段(找 var 引用) ③身份块原始
    //   偏移 ④选中 var 记录区全 dump ⑤info 19位掩码原始 dump。
    println!(
        "=== [V8-0] driver version: {:?} ===",
        nvapi::driver_version()
    );

    // [V8-1] 引擎活表全字段 dump(Var@136 / Equ@104,76B 步进,各 16 条)——
    // 记录内除 type/min/max/value 外的字段可能携带 var 索引引用
    {
        println!("=== [V8-1] engine live tables full-field dump (Var@136 / Equ@104, 76B x16) ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeEquGetControl, NvAPI_GPU_PerfVfeVarGetControl};
        let mut small = vec![0u8; 85016];
        small[0..4].copy_from_slice(&85016u32.to_le_bytes());
        for d in 0..8usize {
            small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) };
        let ident1024: Vec<u8> = small[0..1024].to_vec();

        let mut varb = vec![0u8; 40904];
        varb[0..4].copy_from_slice(&171976u32.to_le_bytes());
        for i in 0..25 {
            varb[4 + i] = 0xFF;
        }
        varb[36..1060].copy_from_slice(&ident1024);
        unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, varb.as_mut_ptr().cast()) };
        let mut eqb = vec![0u8; 1410116];
        eqb[0..4].copy_from_slice(&1410116u32.to_le_bytes());
        eqb[4..1028].copy_from_slice(&ident1024);
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, eqb.as_mut_ptr().cast()) };

        for (tag, buf, base) in [("Var", &varb, 136usize), ("Equ", &eqb, 104usize)] {
            println!("  --- {tag} table @+{base} stride 76 ---");
            for k in 0..16usize {
                let r = base + 76 * k;
                if r + 76 > buf.len() {
                    break;
                }
                let mut fields = Vec::new();
                for (i, w) in buf[r..r + 76].chunks_exact(4).enumerate() {
                    let v = u32::from_le_bytes(w.try_into().unwrap());
                    if v != 0 {
                        let f = f32::from_le_bytes(v.to_le_bytes());
                        fields.push(format!("+{:02X}:{v:#010x}(f32 {f:.3?})", i * 4));
                    }
                }
                if !fields.is_empty() {
                    println!("    r{k}@{r}: {}", fields.join(" "));
                }
            }
        }
    }

    // [V8-2] 身份块 @100/176/252/328/404 原始非零偏移(第七轮只解析了 @136
    // 记录区,漏了它们的内容位置)
    {
        println!("=== [V8-2] per-identity raw nonzero offsets ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeEquGetControl, NvAPI_GPU_PerfVfeVarGetControl};
        let mut small = vec![0u8; 85016];
        small[0..4].copy_from_slice(&85016u32.to_le_bytes());
        for d in 0..8usize {
            small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) };
        for base in [100usize, 176, 252, 328, 404] {
            let end = (base + 1024).min(small.len());
            let ident = small[base..end].to_vec();
            let mut req = vec![0u8; 40904];
            req[0..4].copy_from_slice(&171976u32.to_le_bytes());
            for i in 0..25 {
                req[4 + i] = 0xFF;
            }
            let n = ident.len().min(1024);
            req[36..36 + n].copy_from_slice(&ident[..n]);
            unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) };
            let nz = nonzero_dwords(&req, usize::MAX);
            let body: Vec<String> = nz
                .iter()
                .filter(|(off, _)| *off >= 36)
                .take(24)
                .map(|(off, v)| {
                    let f = f32::from_le_bytes(v.to_le_bytes());
                    format!("+{off:04X}:{v:#010x}(f32 {f:.3?})")
                })
                .collect();
            println!("  ident@{base}: nz={} {}", nz.len(), body.join(" "));
        }
    }

    // [V8-3] 选中 var 记录区全 dump(单 bit+身份):var0/var3/var45 —— 验证
    // round-4 位置回填 + 是否带 value/applied
    {
        println!("=== [V8-3] selected-var record region dump (single-bit + identity) ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeEquGetControl, NvAPI_GPU_PerfVfeVarGetControl};
        let mut small = vec![0u8; 85016];
        small[0..4].copy_from_slice(&85016u32.to_le_bytes());
        for d in 0..8usize {
            small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) };
        let ident1024: Vec<u8> = small[0..1024].to_vec();
        for idx in [0usize, 1, 2, 3, 4, 5, 6, 7, 45] {
            let mut req = vec![0u8; 40904];
            req[0..4].copy_from_slice(&171976u32.to_le_bytes());
            req[4 + idx / 8] = 1 << (idx % 8);
            req[36..1060].copy_from_slice(&ident1024);
            unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) };
            let rec = 104 + 160 * idx;
            let mut fields = Vec::new();
            for (i, w) in req[rec..rec + 160].chunks_exact(4).enumerate() {
                let v = u32::from_le_bytes(w.try_into().unwrap());
                if v != 0 {
                    let f = f32::from_le_bytes(v.to_le_bytes());
                    fields.push(format!("+{:02X}:{v:#010x}(f32 {f:.3?})", i * 4));
                }
            }
            println!(
                "  var{idx} rec@{rec}: {}",
                if fields.is_empty() {
                    "(empty)".to_string()
                } else {
                    fields.join(" ")
                }
            );
        }
    }

    // [V8-4] info 19 位掩码(0x7FFFF)原始 dump:条目基址非均匀(296/572/848/
    // 884/1916),全前 8KB 非零列出找 name ID;另试 0x7FFFFFF(27 位)/0xFFFFFFFF
    {
        println!("=== [V8-4] Var GetInfo raw dump (masks 0x7FFFF / 0xFFFFF / 0x3FFFFFF) ===");
        use nvapi::sys::api::NvAPI_GPU_PerfVfeVarGetInfo;
        let size = 2856296usize;
        for (label, mask_bytes) in [
            ("0x7FFFF", vec![0xFFu8, 0xFF, 0x07]),
            ("0xFFFFF", vec![0xFFu8, 0xFF, 0x0F]),
            ("0x3FFFFFF", vec![0xFFu8, 0xFF, 0xFF, 0x03]),
        ] {
            let mut b = vec![0u8; size];
            b[0..4].copy_from_slice(&3118440u32.to_le_bytes());
            for (i, m) in mask_bytes.iter().enumerate() {
                b[4 + i] = *m;
            }
            let st = unsafe { NvAPI_GPU_PerfVfeVarGetInfo(handle, b.as_mut_ptr().cast()) } as i32;
            let nz = nonzero_dwords(&b, usize::MAX);
            println!(
                "  mask {label}: st={st} nz={} first24={:?}",
                nz.len(),
                &nz[..nz.len().min(24)]
            );
        }
    }
    // ============ 第九轮(2026-11-15,mVolt apply 流全 RE 后)============
    // RE 判决(mVolt 0.50.13 sub_14028C6F0/B700/9BE0):
    // ①V2 VarControl 记录几何与 round-4 完全一致(float* 索引 40*v+27/28/37/38
    //   = 字节 160*v+108/112/148/152,掩码 @4+v/8)——我们 SET 的就是它;
    // ②mVolt GET 带 32B 身份@4(对象+184),SET=身份版 GET 响应原样回传;
    // ③var 索引来自运行时表(对象+28 步进20×8 字节),由验证流锚定;
    // ④VarGetInfo 真结构 = 255 条 × 11200B @296(296+255*11200=2856296 整);
    //   电压 offset var 签名 = type==13 && byte[+1620]==7 && byte[+1621]==8,
    //   +260 起 336B = per-var 特征区;EquControl @76·equ+1092 的 +4/+6 u16 =
    //   var 关联对。
    // 本臂:VarInfo 11200 步进全表扫描(GET-only)——按签名定 8 个 offset var。
    {
        println!("=== [V9-1] Var GetInfo 11200-stride record scan (offset-var signature) ===");
        use nvapi::sys::api::NvAPI_GPU_PerfVfeVarGetInfo;
        let size = 2856296usize;
        let mut b = vec![0u8; size];
        b[0..4].copy_from_slice(&3118440u32.to_le_bytes());
        let st = unsafe { NvAPI_GPU_PerfVfeVarGetInfo(handle, b.as_mut_ptr().cast()) } as i32;
        println!("GET status={st}");
        let mut hits = Vec::new();
        for k in 0..255usize {
            let base = 296 + 11200 * k;
            if base + 11200 > size {
                break;
            }
            let t = u32::from_le_bytes(b[base..base + 4].try_into().unwrap());
            let b1620 = b[base + 1620];
            let b1621 = b[base + 1621];
            if t == 0 && b1620 == 0 && b1621 == 0 {
                continue;
            }
            let sig = t == 13 && b1620 == 7 && b1621 == 8;
            if sig {
                hits.push(k);
            }
            // 打签名命中 + 少量非零样例(避免刷屏:只打 sig 命中和前 8 个其他)
            if sig || hits.len() + k < 40 {
                let mut fields = Vec::new();
                for (i, w) in b[base..base + 276].chunks_exact(4).enumerate() {
                    let v = u32::from_le_bytes(w.try_into().unwrap());
                    if v != 0 {
                        fields.push(format!("+{:03X}:{v:#x}", i * 4));
                    }
                }
                println!(
                    "  rec{k}@{base}: type={t} b1620={b1620} b1621={b1621} {}{}",
                    if sig { "<— OFFSET-VAR SIG" } else { "" },
                    if fields.is_empty() {
                        String::new()
                    } else {
                        format!(" head=[{}]", fields.join(" "))
                    }
                );
            }
        }
        println!("offset-var signature hits: {hits:?}");
        // Equ 层 var 关联对:@76·equ+1092 的 +4/+6 u16(前 24 条)——需身份
        // (裸 GET 只有 stamp;9BE0 校验用 1024B 身份@4)
        println!("  --- Equ var-link pairs (@76*equ+1092, first 24 nonzero) ---");
        use nvapi::sys::api::NvAPI_GPU_PerfVfeEquGetControl;
        let mut small = vec![0u8; 85016];
        small[0..4].copy_from_slice(&85016u32.to_le_bytes());
        for d in 0..8usize {
            small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) };
        let ident1024: Vec<u8> = small[0..1024].to_vec();
        let mut eb = vec![0u8; 1410116];
        eb[0..4].copy_from_slice(&1410116u32.to_le_bytes());
        eb[4..1028].copy_from_slice(&ident1024);
        unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, eb.as_mut_ptr().cast()) };
        println!(
            "  equ GET richness={}",
            nonzero_dwords(&eb, usize::MAX).len()
        );
        let mut printed = 0usize;
        for e in 0..0x2000usize {
            let base = 1092 + 76 * e;
            if base + 12 > eb.len() {
                break;
            }
            let t = u32::from_le_bytes(eb[base..base + 4].try_into().unwrap());
            if t == 0 {
                continue;
            }
            let l1 = u16::from_le_bytes(eb[base + 4..base + 6].try_into().unwrap());
            let l2 = u16::from_le_bytes(eb[base + 6..base + 8].try_into().unwrap());
            println!("  equ{e}@{base}: type={t} link1={l1} link2={l2}");
            printed += 1;
            if printed >= 24 {
                break;
            }
        }
    }
    // ============ 第十一轮:V11 门矩阵批处理(一次执行跑完所有变体)============
    // 修正:V10 的 rec+148/rec+152 为双重累加错偏(mVolt 绝对偏移 160*v+108/112/148/152
    // 应落 rec+4/8/44/48);V10 未曾运行。本臂:每候选 var x {verbatim+20000uV,
    // verbatim+20(mV 释义)} 逐一 SET+还原验证,+全组 7 var 批量 SET,墙值三查+移动检测。
    if std::env::var("NVOC_ALLOW_VFE_VAR0_WRITE").is_ok() && std::env::var("NVOC_VFE_MVOLT_FLOW").is_ok() {
        println!("=== [V11] gate-matrix batch: sig vars [4,5,11,12,13,19,50] x (verbatim uV/mV) + group batch SET ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeEquGetControl, NvAPI_GPU_PerfVfeVarGetControl, NvAPI_GPU_PerfVfeVarSetControl};
        let candidates = [4usize, 5, 11, 12, 13, 19, 50];
        let walls = || -> [i32; 6] {
            gpu.volt_rails()
                .ok()
                .and_then(|r| r.status.first().map(|s| s.values))
                .unwrap_or([0; 6])
        };
        let get_ident = || -> Vec<u8> {
            let mut small = vec![0u8; 85016];
            small[0..4].copy_from_slice(&85016u32.to_le_bytes());
            for d in 0..8usize {
                small[4 + 4 * d..8 + 4 * d].copy_from_slice(&u32::MAX.to_le_bytes());
            }
            unsafe { NvAPI_GPU_PerfVfeEquGetControl(handle, small.as_mut_ptr().cast()) };
            small[0..32].to_vec()
        };
        let get_ctrl = |ident: &[u8]| -> Vec<u8> {
            let mut b = vec![0u8; 40904];
            b[0..4].copy_from_slice(&171976u32.to_le_bytes());
            b[4..36].copy_from_slice(&ident[..32]);
            unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, b.as_mut_ptr().cast()) };
            b
        };
        // ①6220B 第二对象 GET(stamp 137292 —— 接口归属未证实,用 VarGetControl 入口探)
        let mut st6220 = vec![0u8; 6220];
        st6220[0..4].copy_from_slice(&137292u32.to_le_bytes());
        let st6 = unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, st6220.as_mut_ptr().cast()) } as i32;
        println!(
            "  [6220B] st={st6} nz={} head={:?}",
            nonzero_dwords(&st6220, usize::MAX).len(),
            &st6220[4..24]
        );
        let ident0 = get_ident();
        let probe0 = get_ctrl(&ident0);
        println!("  [ident32 echo@4..36] {:?}", &probe0[4..36]);
        println!("  [rec4 dump] type={} rec@744 bytes[0..16]={:?}", u32::from_le_bytes(probe0[744..748].try_into().unwrap()), &probe0[744..760]);

        let mut moved: Vec<String> = Vec::new();
        let mut aborted = false;
        'outer: for idx in candidates {
            for (unit, value) in [("uV", 20000.0f32), ("mV", 20.0)] {
                if aborted {
                    break 'outer;
                }
                let rec = 104 + 160 * idx;
                let w_pre = walls();
                let ident = get_ident();
                let mut req = get_ctrl(&ident);
                let f_ap = req[rec + 44];
                // patch: 掩码 OR + min/max(±clamp)/applied/value(rec+4/8/44/48)
                req[4 + idx / 8] |= 1 << (idx % 8);
                req[rec + 4..rec + 8].copy_from_slice(&value.min(0.0).to_le_bytes());
                req[rec + 8..rec + 12].copy_from_slice(&value.max(0.0).to_le_bytes());
                req[rec + 44] = 1;
                req[rec + 48..rec + 52].copy_from_slice(&value.to_le_bytes());
                let st_s = unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, req.as_ptr().cast()) } as i32;
                std::thread::sleep(std::time::Duration::from_millis(600));
                let back = get_ctrl(&ident);
                let rd_ap = back[rec + 44];
                let rd_v = f32::from_le_bytes(back[rec + 48..rec + 52].try_into().unwrap());
                let w_mid = walls();
                // 还原:工厂 GET 原样 SET 回
                let rst = get_ctrl(&ident);
                unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, rst.as_ptr().cast()) };
                std::thread::sleep(std::time::Duration::from_millis(600));
                let back2 = get_ctrl(&ident);
                let rs_ap = back2[rec + 44];
                let w_post = walls();
                let moved_flag = !w_pre.is_empty()
                    && w_pre.len() == w_mid.len()
                    && (1..w_pre.len()).any(|i| w_pre[i] != w_mid[i]);
                if moved_flag {
                    moved.push(format!("var{idx}[{unit}]"));
                }
                println!(
                    "  var{idx}[{unit}]: SET={st_s} rd(ap={rd_ap},v={rd_v}) rst_ap={rs_ap} walls_pre={w_pre:?} walls_mid={w_mid:?} {}",
                    if moved_flag { "<— WALL MOVED" } else { "" }
                );
                if rs_ap != 0 && f_ap == 0 {
                    println!("  <— RESTORE FAILED — aborting remaining steps (reboot recovers)");
                    aborted = true;
                }
            }
        }
        // ②全组批量 SET:7 var 同一缓冲,7 掩码位,applied=1 value=+20000
        if !aborted {
            let w_pre = walls();
            let ident = get_ident();
            let mut req = get_ctrl(&ident);
            for idx in candidates {
                let rec = 104 + 160 * idx;
                req[4 + idx / 8] |= 1 << (idx % 8);
                req[rec + 4..rec + 8].copy_from_slice(&20000.0f32.min(0.0).to_le_bytes());
                req[rec + 8..rec + 12].copy_from_slice(&20000.0f32.to_le_bytes());
                req[rec + 44] = 1;
                req[rec + 48..rec + 52].copy_from_slice(&20000.0f32.to_le_bytes());
            }
            let st_s = unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, req.as_ptr().cast()) } as i32;
            std::thread::sleep(std::time::Duration::from_millis(800));
            let w_mid = walls();
            let rst = get_ctrl(&ident);
            unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, rst.as_ptr().cast()) };
            std::thread::sleep(std::time::Duration::from_millis(800));
            let mut restored_ok = true;
            for idx in candidates {
                let back2 = get_ctrl(&ident);
                if back2[104 + 160 * idx + 44] != 0 {
                    restored_ok = false;
                }
            }
            let w_post = walls();
            let moved_flag = !w_pre.is_empty()
                && w_pre.len() == w_mid.len()
                && (1..w_pre.len()).any(|i| w_pre[i] != w_mid[i]);
            if moved_flag {
                moved.push("batch-all7".to_string());
            }
            println!(
                "  batch-all7: SET={st_s} walls_pre={w_pre:?} walls_mid={w_mid:?} restored_ok={restored_ok} {}",
                if moved_flag { "<— WALL MOVED" } else { "" }
            );
        }
        println!("=== [V11] WALL-MOVE SUMMARY: {moved:?} (empty = no variant consumed) ===");
    }
    let _ = ptr::null_mut::<u8>();
    let _ = ptr::null_mut::<u8>();
}

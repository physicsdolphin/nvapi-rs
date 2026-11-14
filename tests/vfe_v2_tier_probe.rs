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

    println!("=== Var GetControl V2 (0x2A008, 40904B, 32B identity@+4) ===");
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

    println!("=== Equ GetControl 0x1584C4 (1410116B, 1024B identity@+4) ===");
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

    // V2 写臂 v3:完整工厂记录 base。单 var GET var0 → 160B 记录原样保留,
    // 只改 min/max/applied/value(type 保留 7 与强制 13 双臂)→ SET → 单 bit 读回。
    if std::env::var("NVOC_ALLOW_VFE_WRITE").is_ok() {
        println!("=== Var V2 full-record SET (factory base, two type variants) ===");
        use nvapi::sys::api::{NvAPI_GPU_PerfVfeVarGetControl, NvAPI_GPU_PerfVfeVarSetControl};
        let walls = |tag: &str| {
            if let Ok(r) = gpu.volt_rails() {
                if let Some(s) = r.status.first() {
                    println!("  [{tag}] rail0 status: {:?}", s.values);
                }
            }
        };
        walls("before");
        let single_get = |idx: usize| -> Vec<u8> {
            let mut req = vec![0u8; 40904];
            req[0..4].copy_from_slice(&171976u32.to_le_bytes());
            req[4 + idx / 8] = 1 << (idx % 8);
            unsafe { NvAPI_GPU_PerfVfeVarGetControl(handle, req.as_mut_ptr().cast()) };
            req
        };
        let factory = single_get(0);
        let rec = 104usize;
        let f_type = u32::from_le_bytes(factory[rec..rec + 4].try_into().unwrap());
        println!(
            "factory var0: type={f_type} min={} max={} applied={} value={} record_bytes_nonzero={}",
            f32::from_le_bytes(factory[rec + 4..rec + 8].try_into().unwrap()),
            f32::from_le_bytes(factory[rec + 8..rec + 12].try_into().unwrap()),
            factory[rec + 44],
            f32::from_le_bytes(factory[rec + 48..rec + 52].try_into().unwrap()),
            factory[rec..rec + 160].iter().filter(|&b| *b != 0).count()
        );

        for (label, force_type) in [("keep-type7", None), ("force-type13", Some(13u32))] {
            let mut req = factory.clone();
            for x in req[4..36].iter_mut() {
                *x = 0;
            }
            req[4] = 1; // 单 bit0
            if let Some(t) = force_type {
                req[rec..rec + 4].copy_from_slice(&t.to_le_bytes());
            }
            let value = 6250.0f32;
            req[rec + 4..rec + 8].copy_from_slice(&value.min(0.0).to_le_bytes());
            req[rec + 8..rec + 12].copy_from_slice(&value.max(0.0).to_le_bytes());
            req[rec + 44] = 1;
            req[rec + 48..rec + 52].copy_from_slice(&value.to_le_bytes());
            let st_set =
                unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, req.as_ptr().cast()) } as i32;
            std::thread::sleep(std::time::Duration::from_millis(150));
            let back = single_get(0);
            let rd_t = u32::from_le_bytes(back[rec..rec + 4].try_into().unwrap());
            let rd_ap = back[rec + 44];
            let rd_v = f32::from_le_bytes(back[rec + 48..rec + 52].try_into().unwrap());
            println!(
                "  [{label}] SET={st_set} readback: type={rd_t} applied={rd_ap} value={rd_v} {}",
                if rd_ap == 1 {
                    "<— RETAINED"
                } else {
                    "(dropped)"
                }
            );
            // 还原工厂
            unsafe { NvAPI_GPU_PerfVfeVarSetControl(handle, factory.as_ptr().cast()) };
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        walls("after");
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
    let _ = ptr::null_mut::<u8>();
}

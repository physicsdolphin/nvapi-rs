//! BIOS-override 引擎五接口探针（mVolt+ 0.50.13 RE 定案，2026-11-15）。
//!
//! 六真 QI ID（85 槽 VM 解码定谳——key 表只是校验种子；全部存在于
//! 621.01/621.24 impl 明文表）：
//!   a1[156] 0xB9DA41D6 策略详表 GET 2,856,296B stamp 0x2F9568
//!            （255×11200B 记录自 +296，头 dword=13，tag 7/8 @+1620/+1621）
//!   a1[157] 0x5D387298 BIOS 限制表 GET 40,904B stamp 0x29FC8
//!            （255×160B=40-f32 记录自 +104；+4 = 32B 记录在场位图）
//!   a1[158] 0x79FA23A2 BIOS 限制表 SET = "EXEC cmd-28"——实为 QI fnptr
//!            (handle, buf) 直调，无额外传输头；mVolt 校验器（sub_1401D32C0
//!            case 28）强制 len==40904 + stamp + 位图恰好单记录置位
//!   a1[159] 0x8D49471C 域表基础 GET 623,684B stamp 0xD8444（8192×76B 自+1092）
//!   a1[160] 0x4C75C9FE 域表扩展 GET 1,410,116B stamp=0x158444
//!            （8192×172B 自+1092，f32 min/max/step @+1096/+1100；+4 1KB 位图/身份）
//!   a1[161] 0x2C73AFDC VoltRails 族 GET 6,220B stamp 0x2184C
//!
//! 160B 记录（=40×f32）写面：[27]=负钳 / [28]=正钳 / [37]=applied byte /
//! [38]=value（mVolt apply sub_14028C6F0 patch 点，±1,250,000µV 前置硬钳）。
//! stamp 约定 (ver<<16)|size：0x29FC8=ver2|0x9FC8(40904)。
//!
//! Run: cargo test --release --test bios_override_cmd28_probe -- --ignored --nocapture
//! 默认 GET-only dump；echo 写臂（原值回写，验证 SET 车道通不通）需
//! NVOC_ALLOW_BIOS_LIMIT_WRITE=1。

#![allow(unused_must_use)]

use nvapi::PhysicalGpu;
use nvapi::sys::handles::NvPhysicalGpuHandle;

type Fn2 = unsafe extern "C" fn(NvPhysicalGpuHandle, *mut core::ffi::c_void) -> i32;

fn qi_fn2(id: u32) -> Option<Fn2> {
    match nvapi::sys::nvapi_QueryInterface(id) {
        Ok(p) if !p.is_null() => Some(unsafe { core::mem::transmute(p) }),
        _ => None,
    }
}

fn u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

fn f32_at(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

fn set_bits(buf: &[u8], bitmap_off: usize, bitmap_len: usize, cap: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for byte_i in 0..bitmap_len {
        let b = buf[bitmap_off + byte_i];
        for bit in 0..8 {
            if b & (1 << bit) != 0 {
                out.push(byte_i * 8 + bit);
                if out.len() >= cap {
                    return out;
                }
            }
        }
    }
    out
}

fn get_table(
    f: Fn2,
    handle: NvPhysicalGpuHandle,
    stamp: u32,
    size: usize,
    name: &str,
) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; size];
    buf[0..4].copy_from_slice(&stamp.to_le_bytes());
    // mVolt 请求带 engine identity（32B@+4 / 1KB@+4）；来源未解——先全零
    // 探路：多数 mVolt 车道只 echo 请求域，身份若必须非零会在这里暴露。
    let st = unsafe { f(handle, buf.as_mut_ptr().cast()) };
    println!("  GET {name}: status={st}");
    if st != 0 {
        println!("  GET {name}: FAILED (identity gate? mVolt carries engine identity @+4)");
        return None;
    }
    Some(buf)
}

#[test]
#[ignore]
fn bios_override_cmd28_probe() {
    nvapi::initialize().expect("init");
    let mut gpus = PhysicalGpu::enumerate().expect("enumerate");
    let gpu = gpus.drain(..).next().expect("no gpu");
    let handle = *gpu.handle();
    println!("GPU: {:?}", gpu.full_name());
    println!("driver: {:?}", nvapi::driver_version());

    let limits_get = match qi_fn2(0x5D387298) {
        Some(f) => f,
        None => {
            println!("QueryInterface(0x5D387298) = null — driver predates the lane");
            return;
        }
    };
    let limits_set = qi_fn2(0x79FA23A2);
    println!(
        "QI: GET=ok, SET(0x79FA23A2)={}",
        if limits_set.is_some() { "ok" } else { "NULL" }
    );

    // ---- ① BIOS 限制表 40904B（主表：位图 + 记录 dump）----
    println!("=== [1] BIOS limits GET (0x5D387298, 40904B, stamp 0x29FC8) ===");
    let limits = match get_table(limits_get, handle, 0x29FC8, 40904, "limits") {
        Some(v) => v,
        None => return,
    };
    println!("  stamp echo: {:#x}", u32_at(&limits, 0));
    let present = set_bits(&limits, 4, 32, 300);
    println!(
        "  bitmap@+4: {} record(s) present: {:?}",
        present.len(),
        present
    );
    for &k in present.iter().take(16) {
        let e = 104 + 160 * k;
        if e + 160 > limits.len() {
            break;
        }
        let f27 = f32_at(&limits, e + 27 * 4);
        let f28 = f32_at(&limits, e + 28 * 4);
        let f37 = limits[e + 37 * 4];
        let f38 = f32_at(&limits, e + 38 * 4);
        print!("  rec {k:3} @+{e}: ",);
        for i in 0..4 {
            print!("d{i}={:#010x} ", u32_at(&limits, e + i * 4));
        }
        print!("f0..3=");
        for i in 0..4 {
            print!("{:e} ", f32_at(&limits, e + i * 4));
        }
        println!("f27={f27:e} f28={f28:e} b37={f37} f38={f38:e}");
    }

    // ---- ② 策略详表 2.86MB（头 + 位图 + 首条在场记录 tag 7/8）----
    println!("=== [2] policy detail GET (0xB9DA41D6, 2856296B, stamp 0x2F9568) ===");
    match qi_fn2(0xB9DA41D6) {
        Some(f) => {
            if let Some(det) = get_table(f, handle, 0x2F9568, 2856296, "detail") {
                let bits = set_bits(&det, 4, 32, 8);
                println!("  bitmap@+4 first set: {bits:?}");
                for &k in bits.iter().take(2) {
                    let r = 296 + 11200 * k;
                    if r + 1700 < det.len() {
                        println!(
                            "  rec {k}: hdr={:#x} tag1620={} tag1621={}",
                            u32_at(&det, r),
                            det[r + 1620],
                            det[r + 1621]
                        );
                    }
                }
            }
        }
        None => println!("  QI(0xB9DA41D6) = null"),
    }
}

#[test]
#[ignore]
fn bios_override_domain_and_rail_probe() {
    nvapi::initialize().expect("init");
    let mut gpus = PhysicalGpu::enumerate().expect("enumerate");
    let gpu = gpus.drain(..).next().expect("no gpu");
    let handle = *gpu.handle();
    println!("GPU: {:?}", gpu.full_name());

    // ---- ③ 域表基础 623,684B（8192×76B）----
    println!("=== [3] domain basic GET (0x8D49471C, 623684B, stamp 0xD8444) ===");
    if let Some(f) = qi_fn2(0x8D49471C) {
        if let Some(b) = get_table(f, handle, 0xD8444, 623684, "domain-basic") {
            let bits = set_bits(&b, 4, 1024, 1 << 20);
            println!("  domains present: {}", bits.len());
            println!("  first 24: {:?}", &bits[..bits.len().min(24)]);
            for &d in bits.iter().take(8) {
                let e = 1092 + 76 * d;
                if e + 12 < b.len() {
                    println!(
                        "  dom {d}: hdr={:#x} id={:#x}/{:#x} d3={:#x}",
                        u32_at(&b, e),
                        u32_at(&b, e + 4) & 0xFFFF,
                        u32_at(&b, e + 6) & 0xFFFF,
                        u32_at(&b, e + 8)
                    );
                }
            }
        }
    } else {
        println!("  QI(0x8D49471C) = null");
    }

    // ---- ④ 域表扩展 1,410,116B（8192×172B，float min/max/step）----
    println!("=== [4] domain ext GET (0x4C75C9FE, 1410116B, stamp 0x158444) ===");
    if let Some(f) = qi_fn2(0x4C75C9FE) {
        if let Some(b) = get_table(f, handle, 0x158444, 1410116, "domain-ext") {
            let bits = set_bits(&b, 4, 1024, 1 << 20);
            println!("  domains present: {}", bits.len());
            for &d in bits.iter().take(12) {
                let e = 1092 + 172 * d;
                if e + 52 + 4 < b.len() {
                    println!(
                        "  dom {d:4}: hdr={:#x} f@+4={:e} f@+8={:e} f@+44={:e} f@+48={:e} f@+52={:e}",
                        u32_at(&b, e),
                        f32_at(&b, e + 4),
                        f32_at(&b, e + 8),
                        f32_at(&b, e + 44),
                        f32_at(&b, e + 48),
                        f32_at(&b, e + 52)
                    );
                }
            }
        }
    } else {
        println!("  QI(0x4C75C9FE) = null");
    }

    // ---- ⑤ VoltRails 族 6,220B ----
    println!("=== [5] voltrails-family GET (0x2C73AFDC, 6220B, stamp 0x2184C) ===");
    if let Some(f) = qi_fn2(0x2C73AFDC) {
        if let Some(b) = get_table(f, handle, 0x2184C, 6220, "voltrails") {
            println!("  stamp echo: {:#x}", u32_at(&b, 0));
            let nz: Vec<(usize, u32)> = (0..b.len() / 4)
                .map(|i| (i * 4, u32_at(&b, i * 4)))
                .filter(|(_, v)| *v != 0)
                .take(48)
                .collect();
            for (off, v) in nz {
                println!("  +{off:#06x}: {v:#010x}");
            }
        }
    } else {
        println!("  QI(0x2C73AFDC) = null");
    }
}

#[test]
#[ignore]
fn bios_override_cmd28_echo_write() {
    // 写臂：GET → 位图单记录（原记录原值回写）→ SET 0x79FA23A2 → 再 GET
    // 全量 diff。值不变 = 车道通；任何 diff dword 全部列出。
    // 门控：NVOC_ALLOW_BIOS_LIMIT_WRITE=1。
    if std::env::var("NVOC_ALLOW_BIOS_LIMIT_WRITE").is_err() {
        println!("write arm gated: set NVOC_ALLOW_BIOS_LIMIT_WRITE=1");
        return;
    }
    nvapi::initialize().expect("init");
    let mut gpus = PhysicalGpu::enumerate().expect("enumerate");
    let gpu = gpus.drain(..).next().expect("no gpu");
    let handle = *gpu.handle();
    println!("GPU: {:?}", gpu.full_name());

    let get_f = qi_fn2(0x5D387298).expect("QI GET");
    let set_f = qi_fn2(0x79FA23A2).expect("QI SET (0x79FA23A2) — write lane");
    let before = get_table(get_f, handle, 0x29FC8, 40904, "limits-before").expect("GET");
    let mut req = before.clone();
    // 位图@+4 清零后单记录置位（mVolt cmd-28 校验器要求 v8==1）。
    let Some(target_rec) = set_bits(&before, 4, 32, 1).pop() else {
        println!(
            "VERIFY: limits bitmap empty on this GPU/driver — nothing to echo-write \
             (Turing returns zero records; Blackwell/Ampere carries the BIOS-limits table)"
        );
        return;
    };
    for b in req.iter_mut().skip(4).take(32) {
        *b = 0;
    }
    req[4 + target_rec / 8] |= 1 << (target_rec % 8);
    let st = unsafe { set_f(handle, req.as_mut_ptr().cast()) };
    println!("SET cmd-28 (rec {target_rec}, echo values): status={st}");
    if st != 0 {
        println!("SET rejected — try sudo/-137 permission gate semantics");
        return;
    }
    let after = get_table(get_f, handle, 0x29FC8, 40904, "limits-after").expect("GET 2");
    let diffs: Vec<(usize, u32, u32)> = (0..before.len() / 4)
        .map(|i| (i * 4, u32_at(&before, i * 4), u32_at(&after, i * 4)))
        .filter(|(_, a, b)| a != b)
        .take(64)
        .collect();
    if diffs.is_empty() {
        println!("VERIFY: echo write retained — SET lane functional, state unchanged");
    } else {
        println!("VERIFY: {} dwords differ (first 64):", diffs.len());
        for (off, a, b) in diffs {
            println!("  +{off:#06x}: {a:#010x} -> {b:#010x}");
        }
    }
}

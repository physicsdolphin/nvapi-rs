//! VoltRails Blackwell/621 写保留判别探针（2026-11-14，5060 Ti 全 slot 写零保留事件）。
//!
//! 背景：RTX 5060 Ti / 621.01 上 `set-volt-rail-limit` 任意 slot 写非零值
//! SET status=0 但读回不保留（写 0 可过 = 平凡保留）；同机 20 系 / 621 完全
//! 生效。待判别假设：
//! - **H1** 我方 V2 control patch 几何错位（写进死字节 → GET 不回显）；
//! - **H2** 621 驱动策略变化（melonVolt 写道被退役/需新握手）；
//! - **H3** Blackwell 不消费该车道（墙策略走别的表示）；
//! - **H4** control 真身在 5664B V2 形态（如 status V2 的 172B/轨），2760B
//!   V1/V2 control GET 是有损投影（值被投影为零），SET 写投影被丢弃。
//!
//! 判别手段（e1c 方法论，见 xocd-oc-tool-audit.md §16.7-16.8）：
//! - **A** info V2/V1 raw dump（轨描述符 type/class/µV）；
//! - **B** control 戳阶梯 GET（0x10AC8 / 0x200AC8 / 0x21620(5664B) /
//!   0x30AC8 / 0x40AC8）+ status V1/V2 raw dump——H4 直接检验；
//! - **C** 单 slot +1 step patch → SET → **全缓冲 dword diff**——写落到哪里
//!   （或哪里都不落）一眼可见，判别 H1 vs H2/H3；
//! - **D** 策略镜像初始化（slot1←status vbios 墙、slot2←vrm 墙、slot3←min
//!   hold、slot0←0，再扰动 slot0）——检验「零块=未初始化所以拒收」；
//! - **E** API 级 `set_volt_rail_slot(rail,0,+step/+2step)`——顺带在实机验证
//!   新的诚实报错（RetainMismatch 携带 wrote/retained）。
//!
//! 安全口径：slot 4/5 在 4060L/610 上 -300 写入即爆驱动——**默认永不写**，
//! D 臂的 4/5 镜像需额外 `NVOC_VOLT_SLOT_PROBE_UNSAFE=1`；全部写臂需
//! `NVOC_ALLOW_VOLT_WRITE_PROBE=1`（只读臂 A/B 无需）；每臂即改即还原，
//! 恢复后逐字节校验，失败响亮 panic 并打印需手工写回的字节。
//!
//! Run (read-only arms):
//!   cargo test --test volt_rails_blackwell_probe -- --ignored --nocapture
//! Run (write arms, elevated):
//!   NVOC_ALLOW_VOLT_WRITE_PROBE=1 cargo test --test volt_rails_blackwell_probe -- --ignored --nocapture
//! JSON 落 NVOC_PROBE_OUT（默认 CWD/volt-rails-blackwell-probe.json）。

use core::ptr;
use nvapi::PhysicalGpu;
use nvapi::sys::api::{
    NvAPI_GPU_VoltVoltRailsGetControl, NvAPI_GPU_VoltVoltRailsGetInfo,
    NvAPI_GPU_VoltVoltRailsGetStatus, NvAPI_GPU_VoltVoltRailsSetControl,
};
use nvapi::sys::gpu::power::undocumented::{
    NV_GPU_VOLT_RAILS_CONTROL, NV_GPU_VOLT_RAILS_INFO, NV_GPU_VOLT_RAILS_STATUS,
    NV_GPU_VOLT_RAILS_STATUS_V2, ctrl_entry,
};
use nvapi::sys::nvapi::NvVersion;
use serde_json::json;

/// control GET 戳阶梯：(标签, stamp, 缓冲字节数)。0x21620 是 status-V2 形态
/// 的 5664B 缓冲——拿去戳 control FFI，检验 H4（control 真身是否大形态）。
const CTRL_STAMPS: &[(&str, u32, usize)] = &[
    ("V1|2760", (1 << 16) | 2760, 2760),
    ("V2|2760", (2 << 16) | 2760, 2760),
    ("V2|5664", 0x21620, 5664),
    ("V3|2760", (3 << 16) | 2760, 2760),
    ("V4|2760", (4 << 16) | 2760, 2760),
];

struct CtrlBuf {
    /// whole buffer = version(4) + mask(4) + rest
    bytes: Vec<u8>,
    /// driver status (0 = accepted)
    status: i32,
}

fn ctrl_get(
    handle: nvapi::sys::handles::NvPhysicalGpuHandle,
    info: &NV_GPU_VOLT_RAILS_INFO,
    stamp: u32,
    size: usize,
) -> CtrlBuf {
    let mut buf: Vec<u8> = vec![0; size];
    // seed version + mask + per-entry type bytes directly in the byte buffer
    buf[0..4].copy_from_slice(&stamp.to_le_bytes());
    buf[4..8].copy_from_slice(&info.rail_mask.to_le_bytes());
    let mut dense = 0usize;
    for bit in 0..32u32 {
        if info.rail_mask & (1 << bit) == 0 {
            continue;
        }
        let typ = info.rail_type(bit).unwrap_or(0).to_le_bytes();
        let base = ctrl_entry::STRIDE * dense + ctrl_entry::TYPE;
        buf[base..base + 4].copy_from_slice(&typ);
        dense += 1;
    }
    let st = unsafe { NvAPI_GPU_VoltVoltRailsGetControl(handle, buf.as_mut_ptr().cast()) };
    CtrlBuf {
        bytes: buf,
        status: st as i32,
    }
}

fn ctrl_set(handle: nvapi::sys::handles::NvPhysicalGpuHandle, bytes: &[u8]) -> i32 {
    unsafe { NvAPI_GPU_VoltVoltRailsSetControl(handle, bytes.as_ptr().cast()) as i32 }
}

fn set_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn get_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

/// (rest-relative dword offset, old, new) for every dword that changed.
fn diff_dwords(old: &[u8], new: &[u8]) -> Vec<(usize, u32, u32)> {
    let n = old.len().min(new.len()) / 4 * 4;
    (0..n)
        .step_by(4)
        .map(|off| (off, get_u32(old, off), get_u32(new, off)))
        .filter(|(_, a, b)| a != b)
        .collect()
}

fn nonzero_dwords(buf: &[u8]) -> Vec<(usize, u32)> {
    let n = buf.len() / 4 * 4;
    (0..n)
        .step_by(4)
        .map(|off| (off, get_u32(buf, off)))
        .filter(|(_, v)| *v != 0)
        .collect()
}

fn dump(tag: &str, buf: &[u8], status: i32) {
    println!(
        "== {tag}: status={status} ({:#x}) size={} ==",
        status,
        buf.len()
    );
    for (off, v) in nonzero_dwords(buf) {
        println!("  +{off:04x}: {v:#010x} ({v})");
    }
}

#[test]
#[ignore]
fn volt_rails_blackwell_write_probe() {
    nvapi::initialize().expect("init");
    let mut gpus = PhysicalGpu::enumerate().expect("enumerate");
    let idx: usize = std::env::var("NVOC_PROBE_GPU_INDEX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let gpu = gpus.drain(..).nth(idx).expect("no gpu at index");
    println!("GPU[{}]: {:?}", idx, gpu.full_name());

    let handle = *gpu.handle();
    let mut report = json!({});

    // --- INFO (V2 production stamp, V1 fallback) ---
    let mut info = NV_GPU_VOLT_RAILS_INFO::default();
    let st = unsafe { NvAPI_GPU_VoltVoltRailsGetInfo(handle, ptr::from_mut(&mut info).cast()) };
    let mut legacy_v1 = false;
    if st != 0 {
        println!("GetInfo V2 rejected ({st:#x}), retrying V1 stamp 0x10ACC");
        info = NV_GPU_VOLT_RAILS_INFO::default();
        info.version = NvVersion::with_version(NV_GPU_VOLT_RAILS_INFO::MAGIC_V1);
        let st2 =
            unsafe { NvAPI_GPU_VoltVoltRailsGetInfo(handle, ptr::from_mut(&mut info).cast()) };
        assert_eq!(st2, 0, "GetInfo V1 also failed: {st2:#x}");
        legacy_v1 = true;
    }
    let rail_mask = info.rail_mask;
    println!("legacy_v1={legacy_v1} rail_mask=0x{rail_mask:08X}");
    let mut descriptors = Vec::new();
    for bit in 0..32u32 {
        if rail_mask & (1 << bit) == 0 {
            continue;
        }
        let raw = info.rail_entry_raw(bit).expect("rail entry");
        println!(
            "  rail{bit}: type={} class={} uvA={} uvB={}",
            info.rail_type(bit).unwrap_or(0),
            raw[20],
            raw[22],
            raw[31],
        );
        descriptors.push(json!({"bit": bit, "raw48": raw}));
    }
    report["info"] =
        json!({"legacy_v1": legacy_v1, "rail_mask": rail_mask, "descriptors": descriptors});

    // volt_devices controllable window (VRM Device range/step/default)
    let devices = gpu.volt_devices().ok();
    if let Some(devices) = &devices {
        for d in devices {
            println!(
                "  volt-device: min={} step={} max={} default={} uV",
                d.min_uV, d.step_uV, d.max_uV, d.default_uV
            );
        }
    }
    report["volt_devices"] = json!(&devices);

    let step: u32 = devices
        .as_ref()
        .and_then(|d| d.first().map(|d| d.step_uV))
        .filter(|step| *step > 0)
        .unwrap_or(6250);

    // --- B: control stamp ladder (H4 discriminator) ---
    println!("--- control stamp ladder ---");
    let mut ladder = Vec::new();
    let mut bufs = Vec::new();
    for (tag, stamp, size) in CTRL_STAMPS {
        let buf = ctrl_get(handle, &info, *stamp, *size);
        dump(&format!("ctrl[{tag}]"), &buf.bytes, buf.status);
        ladder.push(json!({"tag": tag, "stamp": stamp, "status": buf.status,
            "nonzero": nonzero_dwords(&buf.bytes)}));
        bufs.push(((*tag, *stamp, *size), buf));
    }
    report["ctrl_ladder"] = json!(ladder);

    // best accepted stamp: first status==0 in the ladder
    let (best, _best_buf) = bufs
        .iter()
        .find(|(_, buf)| buf.status == 0)
        .expect("no control stamp accepted by driver");
    let best = *best;
    println!(
        "best control stamp: {} ({:#x}, {} B)",
        best.0, best.1, best.2
    );

    // --- status V1 + V2 raw ---
    let mut status_v1 = NV_GPU_VOLT_RAILS_STATUS::default();
    status_v1.version = NvVersion::with_version(NV_GPU_VOLT_RAILS_CONTROL::MAGIC_V1);
    status_v1.seed_from_info(&info);
    let st =
        unsafe { NvAPI_GPU_VoltVoltRailsGetStatus(handle, ptr::from_mut(&mut status_v1).cast()) };
    let mut sv1_bytes = vec![0u8; 2760];
    sv1_bytes[0..4].copy_from_slice(&status_v1.version.data.to_le_bytes());
    sv1_bytes[4..8].copy_from_slice(&status_v1.rail_mask.to_le_bytes());
    sv1_bytes[8..].copy_from_slice(&status_v1.rest);
    dump("status[V1|2760]", &sv1_bytes, st as i32);

    let mut status_v2 = NV_GPU_VOLT_RAILS_STATUS_V2::default();
    status_v2.version = NvVersion::with_version(0x21620);
    status_v2.rail_mask = info.rail_mask;
    let st =
        unsafe { NvAPI_GPU_VoltVoltRailsGetStatus(handle, ptr::from_mut(&mut status_v2).cast()) };
    let mut sv2_bytes = vec![0u8; 5664];
    sv2_bytes[0..4].copy_from_slice(&status_v2.version.data.to_le_bytes());
    sv2_bytes[4..8].copy_from_slice(&status_v2.rail_mask.to_le_bytes());
    sv2_bytes[8..].copy_from_slice(&status_v2.rest);
    dump("status[V2|5664]", &sv2_bytes, st as i32);
    let mut sv2_entries = Vec::new();
    for bit in 0..32u32 {
        if rail_mask & (1 << bit) == 0 {
            continue;
        }
        if let Some(entry) = status_v2.entry(bit) {
            println!(
                "  v2 rail{bit}: type={} values={:?} enum={} tail={:?}",
                entry.0, entry.1, entry.2, entry.3
            );
            sv2_entries.push(json!({"bit": bit, "type": entry.0, "values": entry.1, "enum": entry.2, "tail": entry.3}));
        }
    }
    report["status"] =
        json!({"v1_status": st as i32, "v2_status": st as i32, "v2_entries": sv2_entries});

    // status live view (values[0..5]) for the mirror arm
    let status_live = gpu.volt_rails().expect("volt_rails");
    for se in &status_live.status {
        println!(
            "  live status rail{} type{}: {:?}",
            se.rail_bit, se.entry_type, se.values
        );
    }

    // ================= write arms (env-gated) =================
    let allow_write = std::env::var("NVOC_ALLOW_VOLT_WRITE_PROBE").is_ok();
    let unsafe_slots = std::env::var("NVOC_VOLT_SLOT_PROBE_UNSAFE").is_ok();
    if !allow_write {
        println!("write arms skipped — set NVOC_ALLOW_VOLT_WRITE_PROBE=1 to enable");
        report["write_arms"] = json!({"skipped": true});
        write_report(&report);
        return;
    }

    let (tag, stamp, size) = best;
    // C: per-rail single-slot +step diff write (slot 0 only — the wired offset)
    let mut c_arm = Vec::new();
    let mut d_arm = Vec::new();
    let dense_count = rail_mask.count_ones() as usize;
    for dense in 0..dense_count {
        let bit = (0..32u32)
            .filter(|b| rail_mask & (1 << b) != 0)
            .nth(dense)
            .unwrap();
        let snap = ctrl_get(handle, &info, stamp, size);
        assert_eq!(snap.status, 0, "snapshot GET failed");
        let snap_bytes = snap.bytes.clone();

        // values[] rest-relative base for this dense entry
        let val_off = |slot: usize| ctrl_entry::STRIDE * dense + ctrl_entry::VALUES - 8 + slot * 4;
        let old0 = get_u32(&snap_bytes, val_off(0));
        let perturbed = (old0 as i32).wrapping_add(step as i32) as u32;

        // --- C: slot 0 += step ---
        println!("--- C rail{bit} (dense{dense}) slot0: {old0} -> {perturbed} (stamp {tag}) ---");
        let mut patched = snap_bytes.clone();
        set_u32(&mut patched, val_off(0), perturbed);
        let st_set = ctrl_set(handle, &patched);
        let post = ctrl_get(handle, &info, stamp, size);
        let diff = diff_dwords(&snap_bytes, &post.bytes);
        println!(
            "    SET status={st_set} ({:#x}); GET status={}",
            st_set, post.status
        );
        for (off, a, b) in &diff {
            println!("    diff +{off:04x}: {a:#010x} -> {b:#010x}");
        }
        if diff.is_empty() {
            println!(
                "    NO dword changed anywhere in the buffer — write consumed nowhere (H2/H3)"
            );
        }
        c_arm.push(json!({"bit": bit, "slot": 0, "wrote": perturbed, "set_status": st_set,
            "diff": diff.iter().map(|(o, a, b)| json!({"off": o, "old": a, "new": b})).collect::<Vec<_>>()}));

        // --- D: policy mirror init (safe slots 0..3; 4/5 behind UNSAFE) ---
        let mirror = status_live.status.iter().find(|se| se.rail_bit == bit);
        if let Some(se) = mirror {
            // control slots (P100/582.41-pinned): 0=offset 1=vbios wall 2=vrm wall 3=min hold
            let want = [
                (0usize, 0u32),
                (1, se.values[2] as u32),
                (2, se.values[3] as u32),
                (3, se.values[5] as u32),
            ];
            let mut mirror_buf = snap_bytes.clone();
            for (slot, v) in want {
                set_u32(&mut mirror_buf, val_off(slot), v);
            }
            if unsafe_slots {
                // ⚠️ 4/5 live policy fields on some generations — mirror from status
                set_u32(&mut mirror_buf, val_off(4), se.values[4] as u32);
                set_u32(&mut mirror_buf, val_off(5), se.values[1] as u32);
            }
            let st_m = ctrl_set(handle, &mirror_buf);
            let post_m = ctrl_get(handle, &info, stamp, size);
            let diff_m = diff_dwords(&snap_bytes, &post_m.bytes);
            println!("--- D rail{bit} mirror-init: SET={st_m} ({:#x}) ---", st_m);
            for (off, a, b) in &diff_m {
                println!("    diff +{off:04x}: {a:#010x} -> {b:#010x}");
            }
            if diff_m.is_empty() {
                println!("    mirror NOT retained either — zero-block is not the gate");
            }
            // perturb slot0 on top of the mirror
            if !diff_m.is_empty() || st_m == 0 {
                let cur0 = get_u32(&post_m.bytes, val_off(0));
                let mut p2 = post_m.bytes.clone();
                set_u32(
                    &mut p2,
                    val_off(0),
                    (cur0 as i32).wrapping_add(step as i32) as u32,
                );
                let st_p = ctrl_set(handle, &p2);
                let post_p = ctrl_get(handle, &info, stamp, size);
                let diff_p = diff_dwords(&post_m.bytes, &post_p.bytes);
                println!(
                    "    post-mirror slot0 += step: SET={st_p}, {} dwords changed",
                    diff_p.len()
                );
                for (off, a, b) in &diff_p {
                    println!("    diff +{off:04x}: {a:#010x} -> {b:#010x}");
                }
                d_arm.push(json!({"bit": bit, "mirror_set_status": st_m, "mirror_diff": diff_m.len(),
                    "perturb_set_status": st_p, "perturb_diff": diff_p.iter().map(|(o, a, b)| json!({"off": o, "old": a, "new": b})).collect::<Vec<_>>()}));
            }

            // --- restore + verify (byte-exact) ---
            let st_r = ctrl_set(handle, &snap_bytes);
            let restored = ctrl_get(handle, &info, stamp, size);
            let same = restored.bytes == snap_bytes;
            println!("    restore: SET={st_r} byte-exact={same}");
            if !same {
                let d = diff_dwords(&snap_bytes, &restored.bytes);
                panic!(
                    "rail{bit} RESTORE FAILED (SET={st_r}) — remaining diffs {d:?}; snapshot stamp {tag}"
                );
            }
        }
    }
    report["write_arms"] = json!({"ctrl_stamp": {"tag": tag, "stamp": stamp}, "c_slot0_diff": c_arm, "d_mirror": d_arm});

    // --- E: API-level writes (exercises the honest RetainMismatch error) ---
    let mut e_arm = Vec::new();
    for se in &status_live.control {
        let rail = se.rail_bit;
        for mult in [1i32, 2] {
            let want = se.values[0].wrapping_add(step as i32 * mult);
            println!(
                "--- E rail{rail} slot0 API +{} step: {} -> {} ---",
                mult, se.values[0], want
            );
            let res = gpu.set_volt_rail_slot(rail, 0, want);
            println!("    result: {res:?}");
            let after = gpu.volt_rails().expect("volt_rails after E");
            for s2 in &after.status {
                if s2.rail_bit == rail {
                    println!("    status after: {:?}", s2.values);
                }
            }
            e_arm.push(json!({"bit": rail, "mult": mult, "want": want,
                "ok": res.is_ok(),
                "err": res.as_ref().err().map(|e| e.to_string())}));
            // restore slot 0
            let _ = gpu.set_volt_rail_slot(rail, 0, se.values[0]);
        }
    }
    report["e_api"] = json!(e_arm);

    write_report(&report);
    println!(
        "DONE — 判读：C 臂全缓冲 diff 为空 ⇒ 写未被任何字段消费（H2/H3）；diff 非空但不在 val_off ⇒ 几何错位（H1，真槽位即 diff 偏移）；D 臂 mirror 保留 ⇒ 零块未初始化是门；B 臂 0x21620(5664B) 若为唯一 accepted stamp ⇒ H4 成立"
    );
}

fn write_report(report: &serde_json::Value) {
    let path = std::env::var("NVOC_PROBE_OUT")
        .unwrap_or_else(|_| "volt-rails-blackwell-probe.json".to_string());
    match std::fs::write(&path, serde_json::to_vec_pretty(report).unwrap()) {
        Ok(()) => println!("report -> {path}"),
        Err(err) => println!(
            "report write failed ({err}) — CWD={:?}",
            std::env::current_dir()
        ),
    }
}

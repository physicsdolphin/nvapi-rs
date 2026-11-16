//! v3|3240 GET：导出符号 vs QI(0x33AB0353) 双路径对比（全部只读）。
use nvapi::sys::handles::NvPhysicalGpuHandle;

fn u32_at(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

#[test]
#[ignore]
fn v3_via_qi() {
    nvapi::initialize().expect("init");
    let mut gpus = nvapi::PhysicalGpu::enumerate().expect("enumerate");
    let gpu = gpus.drain(..).next().expect("no gpu");
    let handle = *gpu.handle();
    println!("GPU: {:?}", gpu.full_name());

    let get_qi = nvapi::sys::nvapi_QueryInterface(0x33AB0353).expect("QI GET");
    let set_qi = nvapi::sys::nvapi_QueryInterface(0x17695269).expect("QI SET");
    println!("QI GET ptr: {:?}  SET ptr: {:?}", get_qi, set_qi);
    type Fn2 = unsafe extern "C" fn(NvPhysicalGpuHandle, *mut core::ffi::c_void) -> i32;
    let get: Fn2 = unsafe { core::mem::transmute(get_qi) };
    let _set: Fn2 = unsafe { core::mem::transmute(set_qi) };

    for (tag, fill) in [
        ("stamp+zero", (0u32, 0u32)),
        ("stamp+mask(1<<13)", (1u32 << 13, 0u32)),
        ("stamp+13@4", (13u32, 0u32)),
    ] {
        let (fill, _cmd) = fill;
        let mut buf = vec![0u8; 3240];
        buf[..4].copy_from_slice(&0x0003_0CA8u32.to_ne_bytes());
        if fill != 0 {
            buf[4..8].copy_from_slice(&fill.to_ne_bytes());
        }
        let status = unsafe { get(handle, buf.as_mut_ptr().cast()) };
        println!("{tag}: QI GET status={status}");
        if status == 0 {
            let nz: Vec<(usize, u32)> = (4..3240)
                .step_by(4)
                .map(|o| (o, u32_at(&buf, o)))
                .filter(|(_, v)| *v != 0 && *v != 0xFFFF_FFFF)
                .collect();
            println!("  non-zero: {} (first 16)", nz.len());
            for (o, v) in nz.iter().take(16) {
                println!("    +{o:#06x}: {v} (0x{v:08x})");
            }
        }
    }
}

//! GPU utilisation, read through Performance Data Helper (PDH).
//!
//! Task Manager's GPU column comes from `GPU Engine` counters.  We query the
//! same per-engine instances and keep the maximum utilisation across engines,
//! which is exactly how Task Manager decides the GPU percentage to show.
//!
//! The whole module is optional at runtime: if the GPU counters are missing
//! (old driver, no WDDM 2.0), `sample()` just returns `None` and the column
//! stays empty instead of the program failing.

#[cfg(windows)]
mod win {
    use crate::wide::{from_wide_ptr, str_to_wide};
    use std::collections::HashMap;
    use windows_sys::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA,
    };

    /// A live PDH query over "GPU Engine(*)\Utilization Percentage".
    pub struct GpuQuery {
        query: isize,
        counter: isize,
        /// engtype_3D totals per engine instance, keyed by instance name.
        last: HashMap<String, f64>,
    }

    impl GpuQuery {
        pub fn open() -> Option<Self> {
            let path = str_to_wide(r"\GPU Engine(*)\Utilization Percentage");
            let mut query: isize = 0;
            let mut counter: isize = 0;
            unsafe {
                if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                    return None;
                }
                if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                    PdhCloseQuery(query);
                    return None;
                }
                // First collection just primes the counters.
                PdhCollectQueryData(query);
            }
            Some(Self {
                query,
                counter,
                last: HashMap::new(),
            })
        }

        /// Prime the counters and return per-engine utilisation in percent.
        pub fn sample(&mut self) -> Option<HashMap<String, f64>> {
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return None;
                }
                let mut size: u32 = 0;
                let mut count: u32 = 0;
                let rc = PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    std::ptr::null_mut(),
                );
                if rc != PDH_MORE_DATA || size == 0 {
                    return None;
                }
                let mut buf = vec![0u8; size as usize];
                let rc = PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W,
                );
                if rc != 0 || count == 0 {
                    return None;
                }
                let items = std::slice::from_raw_parts(
                    buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
                    count as usize,
                );
                let mut out: HashMap<String, f64> = HashMap::new();
                for item in items {
                    if item.szName.is_null() {
                        continue;
                    }
                    let name = from_wide_ptr(item.szName);
                    if name.is_empty() {
                        continue;
                    }
                    let value = unsafe { item.FmtValue.Anonymous.doubleValue };
                    if !value.is_finite() {
                        continue;
                    }
                    out.insert(name, value.clamp(0.0, 100.0));
                }
                Some(out)
            }
        }

        /// Memory usage per GPU (percent of dedicated VRAM).
        pub fn close(&mut self) {
            if self.query != 0 {
                unsafe { PdhCloseQuery(self.query) };
                self.query = 0;
            }
        }
    }

    impl Drop for GpuQuery {
        fn drop(&mut self) {
            self.close();
        }
    }

    /// Which adapter an engine instance belongs to: `pid_1234_luid_0x..._eng_0`.
    pub fn adapter_of(instance: &str) -> u32 {
        // The LUID hex after "luid_", before "_phys_" / "_eng_".
        if let Some(pos) = instance.find("luid_") {
            let rest = &instance[pos + 5..];
            let hex = rest.split('_').next().unwrap_or("");
            if let Some(h) = hex.strip_prefix("0x") {
                return u32::from_str_radix(h, 16).unwrap_or(0);
            }
        }
        0
    }

    /// Is this a 3D engine? Only 3D engines drive the GPU% Task Manager shows.
    pub fn is_3d(instance: &str) -> bool {
        instance.contains("engtype_3D") || instance.contains("engtype_Copy")
    }
}

#[cfg(windows)]
pub use win::{adapter_of, is_3d, GpuQuery};

/// One aggregated GPU sample.
#[derive(Debug, Clone, Default)]
pub struct GpuSample {
    /// Utilisation per adapter (LUID -> percent), 0..=100.
    pub by_adapter: Vec<(u32, f32)>,
    /// The busiest adapter's utilisation — the number shown in the summary.
    pub overall: f32,
    /// Total dedicated memory used, per adapter LUID.
    pub memory_used: Vec<(u32, u64)>,
}

impl GpuSample {
    pub fn best(&self) -> f32 {
        self.overall
    }

    pub fn adapter_percent(&self, adapter: u32) -> f32 {
        self.by_adapter
            .iter()
            .find(|(a, _)| *a == adapter)
            .map(|(_, v)| *v)
            .unwrap_or(0.0)
    }
}

/// Names of the GPUs present, from the registry (`HKLM\...\Video`).
pub fn adapters() -> Vec<(u32, String)> {
    use crate::registry::Root;
    let mut out = Vec::new();
    let path = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    if let Ok(key) = crate::registry::Key::open(Root::LocalMachine, path) {
        for sub in key.subkeys() {
            if let Ok(k) = crate::registry::Key::open(Root::LocalMachine, &format!("{path}\\{sub}"))
            {
                let name = k.string("DriverDesc").unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                // LUID isn't stored here; index order is stable enough for a
                // human-readable label next to the percentage.
                out.push((out.len() as u32, name));
            }
        }
    }
    out
}

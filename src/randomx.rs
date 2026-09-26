// Safe-ish wrapper over the vendored Shaicoin RandomX.
//
// Shaicoin's fork changes the Argon2 salt to "ShaicoinRandomX-v2\x01" and
// rebalances six instruction frequencies. RANDOMX_FLAG_V2 must also be set or
// the same input hashes differently - with no error anywhere. Verified against
// the known-answer vector in randomx/SHAICOIN-FORK.md; see tests below.

use std::os::raw::{c_int, c_ulong, c_void};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[allow(non_camel_case_types)] pub enum randomx_cache {}
#[allow(non_camel_case_types)] pub enum randomx_dataset {}
#[allow(non_camel_case_types)] pub enum randomx_vm {}

pub const RANDOMX_FLAG_LARGE_PAGES: c_int = 1;
pub const RANDOMX_FLAG_FULL_MEM: c_int = 4;
pub const RANDOMX_FLAG_V2: c_int = 128;

extern "C" {
    fn randomx_get_flags() -> c_int;
    fn randomx_alloc_cache(flags: c_int) -> *mut randomx_cache;
    fn randomx_init_cache(cache: *mut randomx_cache, key: *const c_void, key_size: usize);
    fn randomx_release_cache(cache: *mut randomx_cache);
    fn randomx_alloc_dataset(flags: c_int) -> *mut randomx_dataset;
    fn randomx_init_dataset(ds: *mut randomx_dataset, cache: *mut randomx_cache, start: c_ulong, count: c_ulong);
    fn randomx_dataset_item_count() -> c_ulong;
    fn randomx_release_dataset(ds: *mut randomx_dataset);
    fn randomx_create_vm(flags: c_int, cache: *mut randomx_cache, ds: *mut randomx_dataset) -> *mut randomx_vm;
    fn randomx_destroy_vm(vm: *mut randomx_vm);
    fn randomx_calculate_hash(vm: *mut randomx_vm, input: *const c_void, input_size: usize, output: *mut c_void);
}

/// Cache (+ optional dataset) for one RandomX key. Immutable once built, so it
/// is shared across mining threads; each thread makes its own VM from it.
pub struct RxKey {
    cache: *mut randomx_cache,
    dataset: *mut randomx_dataset,
    flags: c_int,
    pub seed: Vec<u8>,
    pub fast: bool,
    pub large_pages: bool,
}

// Safe: after construction these are only read, and randomx_create_vm /
// randomx_calculate_hash take them as const inputs.
unsafe impl Send for RxKey {}
unsafe impl Sync for RxKey {}

impl RxKey {
    pub fn new(seed: &[u8], want_fast: bool, init_threads: usize) -> Option<Arc<RxKey>> {
        unsafe {
            let base = randomx_get_flags() | RANDOMX_FLAG_V2;

            // Try huge pages first; they are worth 20-30% but need
            // vm.nr_hugepages on the host and are often unavailable.
            let mut large_pages = false;
            let mut flags = base | RANDOMX_FLAG_LARGE_PAGES;
            let mut cache = randomx_alloc_cache(flags);
            if cache.is_null() {
                flags = base;
                cache = randomx_alloc_cache(flags);
            } else {
                large_pages = true;
            }
            if cache.is_null() { return None; }
            randomx_init_cache(cache, seed.as_ptr() as *const c_void, seed.len());

            let mut dataset: *mut randomx_dataset = std::ptr::null_mut();
            let mut fast = false;
            if want_fast {
                let dflags = flags | RANDOMX_FLAG_FULL_MEM;
                dataset = randomx_alloc_dataset(dflags);
                if dataset.is_null() && large_pages {
                    // huge pages can satisfy the cache but not the 2.3 GB dataset
                    dataset = randomx_alloc_dataset(base | RANDOMX_FLAG_FULL_MEM);
                    if !dataset.is_null() { flags = base; large_pages = false; }
                }
                if !dataset.is_null() {
                    let total = randomx_dataset_item_count();
                    let n = init_threads.max(1) as c_ulong;
                    let per = total / n;
                    let ds_addr = dataset as usize;
                    let c_addr = cache as usize;
                    let mut handles = Vec::new();
                    for i in 0..n {
                        let start = i * per;
                        let count = if i == n - 1 { total - start } else { per };
                        handles.push(std::thread::spawn(move || {
                            randomx_init_dataset(ds_addr as *mut randomx_dataset,
                                                 c_addr as *mut randomx_cache, start, count);
                        }));
                    }
                    for h in handles { let _ = h.join(); }
                    flags |= RANDOMX_FLAG_FULL_MEM;
                    fast = true;
                }
            }

            Some(Arc::new(RxKey { cache, dataset, flags, seed: seed.to_vec(), fast, large_pages }))
        }
    }

    pub fn new_vm(self: &Arc<Self>) -> Option<RxVm> {
        unsafe {
            let vm = randomx_create_vm(self.flags, self.cache, self.dataset);
            if vm.is_null() { return None; }
            Some(RxVm { vm, _key: Arc::clone(self) })
        }
    }
}

/// Free `key` as soon as nothing else holds it.
///
/// On a key rotation the old dataset has to be gone before the new one is
/// allocated: a node reserves huge pages for one 2 GB dataset, and allocating
/// the new one first pushed it onto ordinary pages at a ~10% hashrate loss.
/// Workers drop their VMs (and with them their Arc) when they see the key
/// generation change; this waits up to `timeout` for that, then drops the last
/// reference here, which releases the dataset and cache. Returns true if the
/// memory was freed, false if something still held the key at the deadline.
pub fn retire_key(key: Arc<RxKey>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Arc::strong_count(&key) > 1 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let last = Arc::strong_count(&key) == 1;
    drop(key);
    last
}

impl Drop for RxKey {
    fn drop(&mut self) {
        unsafe {
            if !self.dataset.is_null() { randomx_release_dataset(self.dataset); }
            if !self.cache.is_null() { randomx_release_cache(self.cache); }
        }
    }
}

/// One VM, owned by one thread. Holds an Arc to its RxKey so the cache and
/// dataset cannot be freed while this VM still points at them.
pub struct RxVm { vm: *mut randomx_vm, _key: Arc<RxKey> }

impl RxVm {
    #[inline]
    pub fn hash(&mut self, input: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        unsafe {
            randomx_calculate_hash(self.vm, input.as_ptr() as *const c_void,
                                   input.len(), out.as_mut_ptr() as *mut c_void);
        }
        out
    }
}

impl Drop for RxVm {
    fn drop(&mut self) { unsafe { randomx_destroy_vm(self.vm); } }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shaicoin_known_answer_vector() {
        let k = RxKey::new(b"shaicoin-randomx-kat-key-v1", false, 1).expect("cache");
        let mut vm = k.new_vm().expect("vm");
        let got: String = vm.hash(b"shaicoin-randomx-kat-input-v1").iter()
            .map(|b| format!("{:02x}", b)).collect();
        assert_eq!(got, "1e3ede7f49c31a77fe72bc54441f490da2a158a1d8ea7920c2bd46c80ea6bad2",
            "not the Shaicoin RandomX config - a stock build gives \
             d6f97cc88f167c7917c6982b7e8ca5f0d82b8cefbf0242d18b34a5edfe92f265");
    }

    #[test]
    fn retire_key_waits_for_the_last_vm() {
        let k = RxKey::new(b"shaicoin-randomx-kat-key-v1", false, 1).expect("cache");
        // Like a mining worker: the thread builds its own VM (a VM is not Send)
        // and keeps the key alive only through that VM.
        let k2 = Arc::clone(&k);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let held = std::thread::spawn(move || {
            let vm = k2.new_vm().expect("vm");
            drop(k2);
            ready_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            drop(vm);
        });
        ready_rx.recv().unwrap();
        let t = Instant::now();
        assert!(retire_key(k, Duration::from_secs(5)), "the key was not freed");
        assert!(t.elapsed() >= Duration::from_millis(150), "returned before the VM let go");
        held.join().unwrap();
    }

    #[test]
    fn retire_key_gives_up_at_the_deadline() {
        let k = RxKey::new(b"shaicoin-randomx-kat-key-v1", false, 1).expect("cache");
        let vm = k.new_vm().expect("vm");
        assert!(!retire_key(k, Duration::from_millis(100)), "claimed to free a key a VM still holds");
        drop(vm);
    }

    // Needs about 2 x 1170 free 2 MiB huge pages to be meaningful, so it is
    // not run by default:  cargo test --release -- --ignored --nocapture
    #[test]
    #[ignore]
    fn huge_pages_survive_a_key_rotation() {
        let a = RxKey::new(b"rotation-test-key-A", true, 8).expect("dataset A");
        if !a.large_pages { eprintln!("no huge pages for the first dataset; nothing to test"); return; }
        let vm = a.new_vm().expect("vm");
        drop(vm);
        assert!(retire_key(a, Duration::from_secs(5)));
        let b = RxKey::new(b"rotation-test-key-B", true, 8).expect("dataset B");
        assert!(b.large_pages, "the new dataset fell back to ordinary pages after a rotation");
    }
}

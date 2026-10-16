use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

const CACHE_DEPTH: usize = 4;

#[derive(Default)]
struct State {
    sizes: HashMap<PathBuf, u64>,
    pending: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
    closed: bool,
}

pub struct Sizer {
    shared: Arc<(Mutex<State>, Condvar)>,
    pub dirty: Arc<AtomicBool>,
}

impl Sizer {
    pub fn new(workers: usize) -> Sizer {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let dirty = Arc::new(AtomicBool::new(false));
        for _ in 0..workers {
            let (shared, dirty) = (shared.clone(), dirty.clone());
            std::thread::spawn(move || work(&shared, &dirty));
        }
        Sizer { shared, dirty }
    }

    pub fn request(&self, path: &Path) {
        let (lock, cvar) = &*self.shared;
        let Ok(mut st) = lock.lock() else { return };
        if st.closed || st.sizes.contains_key(path) || st.pending.contains(path) {
            return;
        }
        st.pending.insert(path.to_path_buf());
        st.stack.push(path.to_path_buf());
        cvar.notify_one();
    }

    pub fn get(&self, path: &Path) -> Option<u64> {
        self.shared.0.lock().ok()?.sizes.get(path).copied()
    }
}

impl Drop for Sizer {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.shared;
        if let Ok(mut st) = lock.lock() {
            st.closed = true;
            st.stack.clear();
        }
        cvar.notify_all();
    }
}

fn work(shared: &(Mutex<State>, Condvar), dirty: &AtomicBool) {
    let (lock, cvar) = shared;
    loop {
        let path = {
            let Ok(mut st) = lock.lock() else { return };
            while st.stack.is_empty() && !st.closed {
                st = match cvar.wait(st) {
                    Ok(st) => st,
                    Err(_) => return,
                };
            }
            if st.closed {
                return;
            }
            st.stack.pop().expect("stack is not empty")
        };
        walk(shared, &path, 0);
        if let Ok(mut st) = lock.lock() {
            st.pending.remove(&path);
        }
        dirty.store(true, Ordering::Relaxed);
    }
}

fn walk(shared: &(Mutex<State>, Condvar), dir: &Path, depth: usize) -> u64 {
    {
        let Ok(st) = shared.0.lock() else { return 0 };
        if st.closed {
            return 0;
        }
        if let Some(n) = st.sizes.get(dir) {
            return *n;
        }
    }
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                total += walk(shared, &entry.path(), depth + 1);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    if depth <= CACHE_DEPTH
        && let Ok(mut st) = shared.0.lock()
    {
        st.sizes.insert(dir.to_path_buf(), total);
    }
    total
}

#[cfg(test)]
#[path = "sizer_tests.rs"]
mod tests;

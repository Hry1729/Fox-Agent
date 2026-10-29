use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadInterval {
    pub target: PathBuf,
    pub thread: String,
    pub opened_ns: u64,
    pub entered_ns: u64,
    pub exited_ns: u64,
    pub bytes: u64,
    pub read_calls: u64,
    pub completed: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub first_wave: usize,
    pub opened_targets: usize,
    pub gate_released_ns: Option<u64>,
    pub gate_failed: bool,
    pub intervals: Vec<ReadInterval>,
}

#[derive(Default)]
struct State {
    opened: HashSet<PathBuf>,
    gate_deadline: Option<Instant>,
    gate_released_ns: Option<u64>,
    gate_failed: bool,
    intervals: Vec<ReadInterval>,
}

struct Inner {
    targets: HashSet<PathBuf>,
    first_wave: usize,
    gate_timeout: Duration,
    origin: Instant,
    state: Mutex<State>,
    changed: Condvar,
}

fn registry() -> &'static Mutex<Vec<Weak<Inner>>> {
    static ACTIVE: OnceLock<Mutex<Vec<Weak<Inner>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) struct Probe {
    inner: Arc<Inner>,
}

impl Probe {
    pub fn install(targets: &[PathBuf], first_wave: usize, gate_timeout: Duration) -> Result<Self, String> {
        if first_wave == 0 || first_wave > 4 || first_wave > targets.len() || gate_timeout.is_zero() {
            return Err("read probe requires a finite first wave of at most four targets".into());
        }
        let canonical = targets.iter().map(|target| {
            let path = target.canonicalize().map_err(|error| error.to_string())?;
            if !path.is_file() { return Err("read probe target is not an existing file".into()); }
            Ok(path)
        }).collect::<Result<HashSet<_>, String>>()?;
        if canonical.len() != targets.len() { return Err("read probe targets must be distinct canonical files".into()); }
        let inner = Arc::new(Inner {
            targets: canonical, first_wave, gate_timeout, origin: Instant::now(),
            state: Mutex::new(State::default()), changed: Condvar::new(),
        });
        let mut active = registry().lock().map_err(|_| "read probe registry is poisoned")?;
        active.retain(|entry| entry.strong_count() > 0);
        if active.iter().filter_map(Weak::upgrade).any(|other| !other.targets.is_disjoint(&inner.targets)) {
            return Err("another read probe already owns one of these canonical targets".into());
        }
        active.push(Arc::downgrade(&inner));
        Ok(Self { inner })
    }

    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let state = self.inner.state.lock().map_err(|_| "read probe state is poisoned")?;
        Ok(Snapshot {
            first_wave: self.inner.first_wave, opened_targets: state.opened.len(),
            gate_released_ns: state.gate_released_ns, gate_failed: state.gate_failed,
            intervals: state.intervals.clone(),
        })
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Ok(mut active) = registry().lock() {
            active.retain(|entry| entry.upgrade().is_some_and(|other| !Arc::ptr_eq(&other, &self.inner)));
        }
    }
}

pub(super) struct ReadSpan {
    inner: Arc<Inner>,
    interval: ReadInterval,
    recorded: bool,
}

impl ReadSpan {
    pub(super) fn record_read(&mut self, count: usize) {
        self.interval.read_calls += 1;
        self.interval.bytes += count as u64;
    }

    pub(super) fn finish(&mut self) {
        self.interval.completed = true;
        self.record_end();
    }

    fn record_end(&mut self) {
        if self.recorded { return; }
        self.interval.exited_ns = self.inner.origin.elapsed().as_nanos() as u64;
        self.recorded = true;
        if let Ok(mut state) = self.inner.state.lock() { state.intervals.push(self.interval.clone()); }
    }
}

impl Drop for ReadSpan {
    fn drop(&mut self) { self.record_end(); }
}

// Only registered canonical files opt in; all other readers bypass the gate.
pub(super) fn begin_read(target: &Path) -> Result<Option<ReadSpan>, String> {
    let inner = {
        let mut active = registry().lock().map_err(|_| "read probe registry is poisoned")?;
        active.retain(|entry| entry.strong_count() > 0);
        active.iter().filter_map(Weak::upgrade).find(|probe| probe.targets.contains(target))
    };
    let Some(inner) = inner else { return Ok(None); };
    let opened_ns = inner.origin.elapsed().as_nanos() as u64;
    {
        let mut state = inner.state.lock().map_err(|_| "read probe state is poisoned")?;
        if !state.opened.insert(target.to_path_buf()) { return Err("read probe observed a repeated target execution".into()); }
        let deadline = *state.gate_deadline.get_or_insert_with(|| Instant::now() + inner.gate_timeout);
        if state.gate_released_ns.is_none() && Instant::now() >= deadline {
            state.gate_failed = true;
            inner.changed.notify_all();
        }
        if !state.gate_failed && state.gate_released_ns.is_none() && state.opened.len() >= inner.first_wave {
            state.gate_released_ns = Some(inner.origin.elapsed().as_nanos() as u64);
            inner.changed.notify_all();
        }
        while state.gate_released_ns.is_none() && !state.gate_failed {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.gate_failed = true;
                inner.changed.notify_all();
                break;
            }
            let (next, _) = inner.changed.wait_timeout(state, remaining).map_err(|_| "read probe gate is poisoned")?;
            state = next;
        }
        if state.gate_failed {
            return Err(format!("read probe first wave timed out: opened {}, expected {}", state.opened.len(), inner.first_wave));
        }
    }
    let mut span = ReadSpan {
        inner,
        interval: ReadInterval {
            target: target.to_path_buf(), thread: format!("{:?}", std::thread::current().id()),
            opened_ns, entered_ns: 0, exited_ns: 0, bytes: 0, read_calls: 0, completed: false,
        },
        recorded: false,
    };
    // The rendezvous and probe allocation end before this timestamp and File::read.
    span.interval.entered_ns = span.inner.origin.elapsed().as_nanos() as u64;
    Ok(Some(span))
}

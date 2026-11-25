use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::SqlitePool;

use crate::config::Config;
use crate::direct::DirectInfo;
use crate::feedback::Transcriber;
use crate::hub::Hub;
use crate::play::Play;
use crate::storage::Storage;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub storage: Arc<Storage>,
    pub config: Arc<Config>,
    pub login_limiter: Arc<LoginLimiter>,
    pub hub: Arc<Hub>,
    pub play: Arc<Play>,
    pub transcriber: Transcriber,
    pub direct: Arc<DirectInfo>,
}

impl AppState {
    /// Also starts the feedback recorder and transcriber tasks.
    pub fn new(db: SqlitePool, storage: Storage, config: Arc<Config>, direct: Arc<DirectInfo>) -> Self {
        let (recorder, transcriber) = crate::feedback::start(db.clone(), config.data_dir.clone());
        Self {
            db,
            storage: Arc::new(storage),
            config,
            login_limiter: Arc::new(LoginLimiter::default()),
            hub: Arc::new(Hub::default()),
            play: Arc::new(Play::new(recorder)),
            transcriber,
            direct,
        }
    }
}

/// Slows down password guessing: after `MAX_FAILURES` failed logins for one username within
/// `WINDOW`, further attempts are refused until the window passes.
#[derive(Default)]
pub struct LoginLimiter {
    failures: Mutex<HashMap<String, (u32, Instant)>>,
}

const MAX_FAILURES: u32 = 10;
const WINDOW: Duration = Duration::from_secs(15 * 60);

impl LoginLimiter {
    pub fn is_blocked(&self, username: &str) -> bool {
        let map = self.failures.lock().unwrap();
        matches!(map.get(&username.to_lowercase()), Some((n, since)) if *n >= MAX_FAILURES && since.elapsed() < WINDOW)
    }

    pub fn record_failure(&self, username: &str) {
        let mut map = self.failures.lock().unwrap();
        let entry = map.entry(username.to_lowercase()).or_insert((0, Instant::now()));
        if entry.1.elapsed() >= WINDOW {
            *entry = (0, Instant::now());
        }
        entry.0 += 1;
        if map.len() > 10_000 {
            map.retain(|_, (_, since)| since.elapsed() < WINDOW);
        }
    }

    pub fn record_success(&self, username: &str) {
        self.failures.lock().unwrap().remove(&username.to_lowercase());
    }
}

use std::future::Future;
use std::sync::{mpsc, Arc};
use std::task::Context;
use std::time::Duration;

use crate::database::Database;

pub(super) struct TestHome {
    previous: Option<std::ffi::OsString>,
    native_overrides: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _directory: tempfile::TempDir,
}

impl TestHome {
    pub(super) fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", directory.path());
        let native_overrides = [
            ("KIMI_CODE_HOME", ".kimi-code"),
            ("MINIMAX_DATA_DIR", ".minimax"),
            ("DSH_HOME", ".dsh"),
        ]
        .into_iter()
        .map(|(key, dir)| {
            let previous = std::env::var_os(key);
            std::env::set_var(key, directory.path().join(dir));
            (key, previous)
        })
        .collect();
        crate::settings::reload_settings().unwrap();
        Self {
            previous,
            native_overrides,
            _directory: directory,
        }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        for (key, previous) in &self.native_overrides {
            match previous {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        match &self.previous {
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
        let _ = crate::settings::reload_settings();
    }
}

/// Hold the same connection mutex as session sync, then poll the actual command.
/// It must yield before the lock is released, including on a single-thread runtime.
/// The watchdog only prevents a regression from hanging the test process.
fn while_connection_locked<F: Future>(db: &Arc<Database>, name: &str, command: F) -> F::Output {
    let (locked_tx, locked_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let db = db.clone();
    let holder = std::thread::spawn(move || {
        let _guard = db.conn.lock().unwrap();
        locked_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).is_ok()
    });
    locked_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let mut command = std::pin::pin!(command);
    let first_poll = command
        .as_mut()
        .poll(&mut Context::from_waker(futures::task::noop_waker_ref()));
    let _ = release_tx.send(());
    let released_by_caller = holder.join().unwrap();
    assert!(
        released_by_caller,
        "{name} blocked its polling thread on the database lock"
    );
    assert!(
        first_poll.is_pending(),
        "{name} must wait asynchronously for the database"
    );

    tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(5), command)
            .await
            .unwrap_or_else(|_| panic!("{name} did not complete after releasing the database"))
    })
}

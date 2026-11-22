// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use freeshow_churchapps::{ChurchApps, ImportControl, PreparedSync};

pub struct ChurchImport {
    pub root: PathBuf,
    pub control: Arc<ImportControl>,
    result: mpsc::Receiver<Result<PreparedSync, String>>,
}

impl ChurchImport {
    pub fn start(root: PathBuf) -> std::io::Result<Self> {
        let control = Arc::new(ImportControl::default());
        let (sender, result) = mpsc::channel();
        let worker_root = root.clone();
        let worker_control = Arc::clone(&control);
        std::thread::Builder::new()
            .name("churchapps-import".into())
            .spawn(move || {
                let _ = sender.send(ChurchApps.prepare(&worker_root, &worker_control));
            })?;
        Ok(Self {
            root,
            control,
            result,
        })
    }

    pub fn result(&self) -> Option<Result<PreparedSync, String>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("ChurchApps import stopped".into())),
        }
    }

    pub fn status(&self) -> String {
        const MIB: f64 = 1024.0 * 1024.0;
        if self.control.is_cancelled() {
            return "Cancelling ChurchApps import…".into();
        }
        match self.control.progress() {
            Some(transfer) => {
                let received = transfer.received as f64 / MIB;
                if let Some(total) = transfer.total {
                    format!(
                        "{} · {received:.1} / {:.1} MiB",
                        transfer.label,
                        total as f64 / MIB
                    )
                } else {
                    format!("{} · {received:.1} MiB", transfer.label)
                }
            }
            None => "Checking ChurchApps and Lessons.church…".into(),
        }
    }
}

impl Drop for ChurchImport {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

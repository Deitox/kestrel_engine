use crate::mesh::{Mesh, MeshImport};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc;
use std::thread;

pub(super) struct MeshReloadRequest {
    pub(super) key: String,
    pub(super) path: PathBuf,
}

pub(super) struct MeshReloadJob {
    pub(super) request: MeshReloadRequest,
}

pub(super) struct MeshReloadResult {
    pub(super) key: String,
    pub(super) path: PathBuf,
    pub(super) data: Result<MeshImport>,
}

pub(super) fn run_mesh_reload_job(job: MeshReloadJob) -> MeshReloadResult {
    let MeshReloadJob { request } = job;
    let MeshReloadRequest { key, path } = request;
    let data = Mesh::load_gltf_with_materials(&path);
    MeshReloadResult { key, path, data }
}

pub(super) struct MeshReloadWorker {
    senders: Vec<mpsc::SyncSender<MeshReloadJob>>,
    next_sender: AtomicUsize,
    rx: mpsc::Receiver<MeshReloadResult>,
}

impl MeshReloadWorker {
    pub(super) fn new(queue_depth: usize) -> Option<Self> {
        let worker_count = thread::available_parallelism().map(|n| n.get().clamp(1, 2)).unwrap_or(1);
        let (result_tx, result_rx) = mpsc::channel();
        let mut senders = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (tx, rx) = mpsc::sync_channel(queue_depth);
            let thread_result_tx = result_tx.clone();
            let name = format!("mesh-reload-{index}");
            if thread::Builder::new()
                .name(name)
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let result = run_mesh_reload_job(job);
                        if thread_result_tx.send(result).is_err() {
                            break;
                        }
                    }
                })
                .is_err()
            {
                eprintln!("[mesh] failed to spawn reload worker thread");
                return None;
            }
            senders.push(tx);
        }
        Some(Self { senders, next_sender: AtomicUsize::new(0), rx: result_rx })
    }

    pub(super) fn submit(&self, job: MeshReloadJob) -> std::result::Result<(), MeshReloadJob> {
        if self.senders.is_empty() {
            return Err(job);
        }
        let len = self.senders.len();
        let mut job = job;
        let start = self.next_sender.fetch_add(1, AtomicOrdering::Relaxed) % len;
        for offset in 0..len {
            let idx = (start + offset) % len;
            match self.senders[idx].try_send(job) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Full(returned)) | Err(mpsc::TrySendError::Disconnected(returned)) => {
                    job = returned;
                }
            }
        }
        Err(job)
    }

    pub(super) fn drain(&self) -> Vec<MeshReloadResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            results.push(result);
        }
        results
    }
}

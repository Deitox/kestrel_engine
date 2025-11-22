use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc;
use std::thread;

use anyhow::Result;

use crate::animation_validation::{AnimationValidationEvent, AnimationValidator};
use crate::assets::{self, AnimationClip, AnimationGraphAsset};
use crate::assets::{parse_animation_clip_bytes, parse_animation_graph_bytes};

use super::animation_watch::AnimationAssetKind;
use super::ANIMATION_RELOAD_WORKER_QUEUE_DEPTH;

pub(super) struct AnimationReloadRequest {
    pub(super) path: PathBuf,
    pub(super) key: String,
    pub(super) kind: AnimationAssetKind,
    pub(super) skip_validation: bool,
}

pub(super) struct AnimationReloadJob {
    pub(super) request: AnimationReloadRequest,
}

pub(super) struct AnimationReloadResult {
    pub(super) request: AnimationReloadRequest,
    pub(super) data: Result<AnimationReloadData>,
}

pub(super) enum AnimationReloadData {
    Clip { clip: Box<AnimationClip>, bytes: Vec<u8> },
    Graph { graph: AnimationGraphAsset, bytes: Vec<u8> },
    Skeletal { import: assets::skeletal::SkeletonImport },
}

pub(super) struct AnimationReloadQueue {
    buckets: [VecDeque<AnimationReloadRequest>; AnimationAssetKind::COUNT],
    next_bucket: usize,
    max_len: usize,
}

impl AnimationReloadQueue {
    pub(super) fn new(max_len: usize) -> Self {
        Self { buckets: [VecDeque::new(), VecDeque::new(), VecDeque::new()], next_bucket: 0, max_len }
    }

    pub(super) fn enqueue(&mut self, request: AnimationReloadRequest) -> Option<AnimationReloadRequest> {
        let idx = request.kind.index();
        let bucket = &mut self.buckets[idx];
        let dropped = if bucket.len() >= self.max_len { bucket.pop_front() } else { None };
        bucket.push_back(request);
        dropped
    }

    pub(super) fn push_front(&mut self, request: AnimationReloadRequest) -> Option<AnimationReloadRequest> {
        let idx = request.kind.index();
        let bucket = &mut self.buckets[idx];
        bucket.push_front(request);
        if bucket.len() > self.max_len {
            bucket.pop_back()
        } else {
            None
        }
    }

    pub(super) fn pop_next(&mut self) -> Option<AnimationReloadRequest> {
        for _ in 0..self.buckets.len() {
            let idx = self.next_bucket % self.buckets.len();
            if let Some(request) = self.buckets[idx].pop_front() {
                self.next_bucket = (idx + 1) % self.buckets.len();
                return Some(request);
            }
            self.next_bucket = (idx + 1) % self.buckets.len();
        }
        None
    }
}

pub(super) struct AnimationAssetReload {
    pub(super) path: PathBuf,
    pub(super) kind: AnimationAssetKind,
    pub(super) bytes: Option<Vec<u8>>,
}

pub(super) struct AnimationValidationJob {
    pub(super) path: PathBuf,
    pub(super) kind: AnimationAssetKind,
    pub(super) bytes: Option<Vec<u8>>,
}

pub(super) struct AnimationValidationResult {
    pub(super) path: PathBuf,
    pub(super) kind: AnimationAssetKind,
    pub(super) events: Vec<AnimationValidationEvent>,
}

pub(super) struct AnimationReloadController {
    pending: HashSet<(PathBuf, AnimationAssetKind)>,
    queue: AnimationReloadQueue,
    reload_worker: Option<AnimationReloadWorker>,
    validation_worker: Option<AnimationValidationWorker>,
}

impl AnimationReloadController {
    pub(super) fn new(
        max_pending_per_kind: usize,
        reload_worker: Option<AnimationReloadWorker>,
        validation_worker: Option<AnimationValidationWorker>,
    ) -> Self {
        Self {
            pending: HashSet::new(),
            queue: AnimationReloadQueue::new(max_pending_per_kind),
            reload_worker,
            validation_worker,
        }
    }

    pub(super) fn enqueue(&mut self, request: AnimationReloadRequest) -> Vec<AnimationReloadResult> {
        let pending_key = (request.path.clone(), request.kind);
        if !self.pending.insert(pending_key.clone()) {
            return Vec::new();
        }
        if let Some(evicted) = self.queue.enqueue(request) {
            self.pending.remove(&(evicted.path.clone(), evicted.kind));
        }
        self.dispatch_queue()
    }

    pub(super) fn dispatch_queue(&mut self) -> Vec<AnimationReloadResult> {
        let mut inline_results = Vec::new();
        loop {
            let Some(request) = self.queue.pop_next() else {
                break;
            };
            match self.try_submit_animation_reload(request) {
                Ok(()) => continue,
                Err(request) => {
                    if self.reload_worker.is_some() {
                        if let Some(evicted) = self.queue.push_front(request) {
                            self.pending.remove(&(evicted.path.clone(), evicted.kind));
                        }
                        break;
                    } else {
                        let result = run_animation_reload_job(AnimationReloadJob { request });
                        self.mark_completed(&result);
                        inline_results.push(result);
                    }
                }
            }
        }
        inline_results
    }

    pub(super) fn drain_animation_reload_results(&mut self) -> Vec<AnimationReloadResult> {
        let mut results = Vec::new();
        if let Some(worker) = self.reload_worker.as_ref() {
            for result in worker.drain() {
                self.mark_completed(&result);
                results.push(result);
            }
        }
        results
    }

    pub(super) fn submit_validation_job(
        &self,
        job: AnimationValidationJob,
    ) -> Option<AnimationValidationResult> {
        if let Some(worker) = self.validation_worker.as_ref() {
            match worker.submit(job) {
                Ok(()) => None,
                Err(returned) => Some(run_animation_validation_job(returned)),
            }
        } else {
            Some(run_animation_validation_job(job))
        }
    }

    pub(super) fn drain_validation_results(&self) -> Vec<AnimationValidationResult> {
        if let Some(worker) = self.validation_worker.as_ref() {
            worker.drain()
        } else {
            Vec::new()
        }
    }

    fn try_submit_animation_reload(
        &self,
        request: AnimationReloadRequest,
    ) -> std::result::Result<(), AnimationReloadRequest> {
        if let Some(worker) = self.reload_worker.as_ref() {
            worker.submit(AnimationReloadJob { request }).map_err(|job| job.request)
        } else {
            Err(request)
        }
    }

    fn mark_completed(&mut self, result: &AnimationReloadResult) {
        self.pending.remove(&(result.request.path.clone(), result.request.kind));
    }
}

pub(super) struct AnimationReloadWorker {
    senders: Vec<mpsc::SyncSender<AnimationReloadJob>>,
    next_sender: AtomicUsize,
    rx: mpsc::Receiver<AnimationReloadResult>,
}

impl AnimationReloadWorker {
    pub(super) fn new() -> Option<Self> {
        let worker_count = thread::available_parallelism().map(|n| n.get().clamp(2, 4)).unwrap_or(2);
        let (result_tx, result_rx) = mpsc::channel();
        let mut senders = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (tx, rx) = mpsc::sync_channel(ANIMATION_RELOAD_WORKER_QUEUE_DEPTH);
            let thread_result_tx = result_tx.clone();
            let name = format!("animation-reload-{index}");
            if thread::Builder::new()
                .name(name)
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let result = run_animation_reload_job(job);
                        if thread_result_tx.send(result).is_err() {
                            break;
                        }
                    }
                })
                .is_err()
            {
                eprintln!("[animation] failed to spawn reload worker thread");
                return None;
            }
            senders.push(tx);
        }
        Some(Self { senders, next_sender: AtomicUsize::new(0), rx: result_rx })
    }

    pub(super) fn submit(&self, job: AnimationReloadJob) -> std::result::Result<(), AnimationReloadJob> {
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

    pub(super) fn drain(&self) -> Vec<AnimationReloadResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            results.push(result);
        }
        results
    }
}

pub(super) struct AnimationValidationWorker {
    tx: mpsc::Sender<AnimationValidationJob>,
    rx: mpsc::Receiver<AnimationValidationResult>,
}

impl AnimationValidationWorker {
    pub(super) fn new() -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let builder = thread::Builder::new().name("animation-validation".to_string());
        match builder.spawn(move || {
            while let Ok(job) = rx.recv() {
                let result = run_animation_validation_job(job);
                if result_tx.send(result).is_err() {
                    break;
                }
            }
        }) {
            Ok(_) => Some(Self { tx, rx: result_rx }),
            Err(err) => {
                eprintln!("[animation] failed to spawn validation worker: {err:?}");
                None
            }
        }
    }

    pub(super) fn submit(
        &self,
        job: AnimationValidationJob,
    ) -> std::result::Result<(), AnimationValidationJob> {
        self.tx.send(job).map_err(|err| err.0)
    }

    pub(super) fn drain(&self) -> Vec<AnimationValidationResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            results.push(result);
        }
        results
    }
}

pub(super) fn run_animation_validation_job(job: AnimationValidationJob) -> AnimationValidationResult {
    let AnimationValidationJob { path, kind, bytes } = job;
    let events = match kind {
        AnimationAssetKind::Clip => {
            if let Some(payload) = bytes.as_deref() {
                AnimationValidator::validate_clip_bytes(&path, payload)
            } else {
                AnimationValidator::validate_path(&path)
            }
        }
        AnimationAssetKind::Graph => {
            if let Some(payload) = bytes.as_deref() {
                AnimationValidator::validate_graph_bytes(&path, payload)
            } else {
                AnimationValidator::validate_path(&path)
            }
        }
        AnimationAssetKind::Skeletal => AnimationValidator::validate_path(&path),
    };
    AnimationValidationResult { path, kind, events }
}

pub(super) fn run_animation_reload_job(job: AnimationReloadJob) -> AnimationReloadResult {
    let AnimationReloadJob { request } = job;
    let data = match request.kind {
        AnimationAssetKind::Clip => {
            let bytes = match fs::read(&request.path) {
                Ok(bytes) => bytes,
                Err(err) => return AnimationReloadResult { request, data: Err(err.into()) },
            };
            let label = request.path.to_string_lossy().to_string();
            match parse_animation_clip_bytes(&bytes, &request.key, &label) {
                Ok(clip) => Ok(AnimationReloadData::Clip { clip: Box::new(clip), bytes }),
                Err(err) => Err(err),
            }
        }
        AnimationAssetKind::Graph => {
            let bytes = match fs::read(&request.path) {
                Ok(bytes) => bytes,
                Err(err) => return AnimationReloadResult { request, data: Err(err.into()) },
            };
            let label = request.path.to_string_lossy().to_string();
            match parse_animation_graph_bytes(&bytes, &request.key, &label) {
                Ok(graph) => Ok(AnimationReloadData::Graph { graph, bytes }),
                Err(err) => Err(err),
            }
        }
        AnimationAssetKind::Skeletal => match assets::skeletal::load_skeleton_from_gltf(&request.path) {
            Ok(import) => Ok(AnimationReloadData::Skeletal { import }),
            Err(err) => Err(err),
        },
    };
    AnimationReloadResult { request, data }
}

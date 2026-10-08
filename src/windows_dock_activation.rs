//! Shell activation can load third-party handlers; keep it off the preview pump.
use super::*;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};

pub struct Activations {
    requests: mpsc::SyncSender<(String, Vec<String>)>,
    errors: mpsc::Receiver<String>,
    active: Arc<AtomicBool>,
}
impl Activations {
    pub fn new(wake: Arc<wait::Wake>) -> Result<Self> {
        Self::worker(wake, |id, files| activate_package(id, files).map_err(|e| e.to_string()))
    }
    fn worker(wake: Arc<wait::Wake>, activate: impl Fn(&str, &[String]) -> std::result::Result<(), String> + Send + 'static) -> Result<Self> {
        let (requests, incoming) = mpsc::sync_channel::<(String, Vec<String>)>(4);
        let (send, errors) = mpsc::sync_channel(5);
        let active = Arc::new(AtomicBool::new(true));
        let alive = active.clone();
        std::thread::Builder::new().name("native-dock-activation".into()).spawn(move || {
            while let Ok((id, files)) = incoming.recv() {
                if !alive.load(Ordering::Acquire) { break; }
                if let Err(error) = activate(&id, &files) {
                    if send.send(error).is_err() { break; }
                    wake.signal();
                }
            }
        })?;
        Ok(Self { requests, errors, active })
    }
    pub fn request(&self, id: &str, files: &[String]) -> Result<()> {
        self.requests.try_send((id.to_owned(), files.to_vec())).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => "too many pending application opens; try again after Windows responds",
            mpsc::TrySendError::Disconnected(_) => "the Windows application activation worker stopped",
        })?;
        Ok(())
    }
    pub fn errors(&self) -> Vec<String> { self.errors.try_iter().collect() }
}
impl Drop for Activations {
    fn drop(&mut self) {
        // Already executing OS calls keep their application-owned lifetime.
        // Discard queued opens without waiting on a possibly blocked extension.
        self.active.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    struct Finished(mpsc::Sender<()>);
    impl Drop for Finished { fn drop(&mut self) { let _ = self.0.send(()); } }

    #[test]
    fn blocked_shell_activation_does_not_block_requests_or_shutdown() -> Result<()> {
        let (began, started) = mpsc::channel();
        let (release, waiting) = mpsc::channel();
        let (ended, finished) = mpsc::channel();
        let ended = Finished(ended);
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let caller = std::thread::current().id();
        let worker = Activations::worker(wait::Wake::new()?, move |_, _| {
            let _keep_until_worker_exit = &ended;
            assert_ne!(std::thread::current().id(), caller);
            observed.fetch_add(1, Ordering::SeqCst);
            began.send(()).unwrap();
            waiting.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })?;
        worker.request("owned-test", &[])?;
        started.recv_timeout(Duration::from_secs(5))?;
        for _ in 0..4 { worker.request("owned-test", &[])?; }
        assert!(worker.request("overflow", &[]).is_err());
        drop(worker);
        release.send(())?;
        finished.recv_timeout(Duration::from_secs(5))?;
        assert_eq!(calls.load(Ordering::SeqCst), 1, "queued opens survived shutdown");
        Ok(())
    }

    #[test]
    fn activation_errors_reach_the_preview_without_replaying_the_request() -> Result<()> {
        let worker = Activations::worker(wait::Wake::new()?, |id, files| {
            assert_eq!(id, "owned-test");
            assert_eq!(files, &["C:\\owned ñ.txt"]);
            Err("owned activation refused".into())
        })?;
        worker.request("owned-test", &["C:\\owned ñ.txt".into()])?;
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let errors = worker.errors();
            if !errors.is_empty() {
                assert_eq!(errors, ["owned activation refused"]);
                assert!(worker.errors().is_empty());
                return Ok(());
            }
            assert!(Instant::now() < until, "activation failure was lost");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

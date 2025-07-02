use std::num::NonZero;
use std::thread::available_parallelism;

/// Thread pool that uses [`jod_thread`] to make sure all threads are joined.
pub struct Pool {
    // `_handles` is never read: the field is present
    // only for its `Drop` impl.

    // The worker threads exit once the channel closes;
    // make sure to keep `job_sender` above `handles`
    // so that the channel is actually closed
    // before we join the worker threads!
    job_sender: crossbeam_channel::Sender<Job>,
    _handles: Vec<jod_thread::JoinHandle<()>>,

    parallelism: NonZero<usize>,
}

struct Job {
    f: Box<dyn FnOnce() + Send + 'static>,
}

impl Pool {
    pub fn new(max_threads: usize) -> Pool {
        /// Custom stack size, larger than OS defaults, to avoid stack overflows on platforms with
        /// low stack size defaults.
        const STACK_SIZE: usize = 2 * 1024 * 1024;

        /// The default number of threads in the pool in case system parallelism is not available.
        ///
        /// According to docs, [`available_parallelism`] (almost) only fails when the process is
        /// running with limited permissions.
        /// We are making an assumption here that nowadays it is more probable to run without
        /// necessary permissions on a multicore machine than on a single-core one.
        const DEFAULT_PARALLELISM: usize = 4;

        let threads = available_parallelism()
            .map(usize::from)
            .unwrap_or(DEFAULT_PARALLELISM)
            .min(max_threads);

        let (job_sender, job_receiver) = crossbeam_channel::unbounded();

        let mut handles = Vec::with_capacity(threads);
        for i in 0..threads {
            let handle = jod_thread::Builder::new()
                .stack_size(STACK_SIZE)
                .name(format!("cairo-ls:worker:{i}"))
                .spawn({
                    let job_receiver: crossbeam_channel::Receiver<Job> = job_receiver.clone();
                    move || {
                        for job in job_receiver {
                            (job.f)();
                        }
                    }
                })
                .expect("failed to spawn thread");

            handles.push(handle);
        }

        Pool {
            _handles: handles,
            job_sender,
            parallelism: NonZero::new(threads).unwrap(),
        }
    }

    pub fn spawn<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.send_job(Box::new(move || {
            f();
        }));
    }

    fn send_job(&self, f: Box<dyn FnOnce() + Send + 'static>) {
        let job = Job { f: Box::new(f) };
        self.job_sender.send(job).unwrap();
    }

    /// Returns a number of tasks that this pool can run concurrently.
    pub fn parallelism(&self) -> NonZero<usize> {
        self.parallelism
    }
}

use crate::service::Service;
use crate::signals::Shutdown;
use crate::trace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct Core {
    pub keep_running: AtomicBool,
    services: Mutex<Vec<Arc<dyn Service>>>,
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Core {
    pub fn new() -> Core {
        Core { keep_running: AtomicBool::new(true), services: Mutex::new(Vec::new()) }
    }

    pub fn bind<T>(&self, service: Arc<T>)
    where
        T: Service,
    {
        self.services.lock().unwrap().push(service);
    }

    pub fn run(self: &Arc<Core>) {
        let mut workers = Vec::new();
        for service in self.services.lock().unwrap().iter() {
            workers.append(&mut service.clone().start(self.clone()));
        }
        trace!("core is starting {} workers", workers.len());

        // println!("starting termination...");
        for worker in workers {
            match worker.join() {
                Ok(()) => {}
                Err(err) => {
                    trace!("thread join failure: {:?}", err);
                }
            }
        }

        trace!("... core is shut down");
    }
}

impl Shutdown for Core {
    fn shutdown(self: &Arc<Core>) {
        let keep_running = self.keep_running.load(Ordering::SeqCst);
        if !keep_running {
            return;
        }

        trace!("signaling core shutdown...");
        self.keep_running.store(false, Ordering::SeqCst);

        {
            for service in self.services.lock().unwrap().iter() {
                let ident = service.clone().ident();
                trace!("shutting down: {}", ident);
                service.clone().stop();
            }
        }

        trace!("core is shutting down...");
    }
}

//! Long-running work (creating prefixes, downloading Steam, …) runs on
//! background threads so the UI never blocks. Threads report back over a
//! channel and wake the UI with `request_repaint`.

use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;

pub enum Event {
    /// A line for the activity log.
    Log(String),
    /// Download/installation progress in `0.0..=1.0`, or `None` if unknown.
    Progress(Option<f32>),
    /// A job finished; the UI should reload bottles from disk.
    Finished {
        label: String,
        error: Option<String>,
    },
}

/// Handle given to a job so it can report progress.
#[derive(Clone)]
pub struct Reporter {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl Reporter {
    pub fn log(&self, msg: impl Into<String>) {
        self.send(Event::Log(msg.into()));
    }

    pub fn progress(&self, fraction: Option<f32>) {
        self.send(Event::Progress(fraction));
    }

    fn send(&self, event: Event) {
        // The receiver only goes away when the app is closing.
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

pub struct Jobs {
    tx: Sender<Event>,
    rx: Receiver<Event>,
    pub running: Vec<String>,
}

impl Jobs {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            running: Vec::new(),
        }
    }

    pub fn is_busy(&self) -> bool {
        !self.running.is_empty()
    }

    /// Run `job` on a background thread.
    pub fn spawn<F>(&mut self, ctx: &egui::Context, label: impl Into<String>, job: F)
    where
        F: FnOnce(&Reporter) -> winewrap_core::Result<()> + Send + 'static,
    {
        let label = label.into();
        self.running.push(label.clone());
        let reporter = Reporter {
            tx: self.tx.clone(),
            ctx: ctx.clone(),
        };
        std::thread::spawn(move || {
            let error = job(&reporter).err().map(|e| e.to_string());
            reporter.send(Event::Finished { label, error });
        });
    }

    /// Drain pending events. `Finished` events also update `running`.
    pub fn poll(&mut self) -> Vec<Event> {
        let events: Vec<_> = self.rx.try_iter().collect();
        for e in &events {
            if let Event::Finished { label, .. } = e {
                if let Some(i) = self.running.iter().position(|l| l == label) {
                    self.running.remove(i);
                }
            }
        }
        events
    }
}

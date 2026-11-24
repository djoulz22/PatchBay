use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};

use super::{Capture, Offline, prime_output};
use crate::audio::{self, AudioOutput};
use crate::browser_audio::BrowserCapture;
use crate::graph::{GraphNode, NodeKind};

pub(super) struct EndpointSpec {
    node: GraphNode,
    sample_rate: u32,
    channels: usize,
}

impl EndpointSpec {
    pub(super) fn new(node: &GraphNode, offline: &Offline, channels: usize) -> Self {
        Self {
            node: node.clone(),
            sample_rate: offline.sample_rate,
            channels,
        }
    }

    fn device_id(&self) -> Option<&str> {
        match &self.node.kind {
            NodeKind::DeviceInput { device_id } | NodeKind::DeviceOutput { device_id } => {
                Some(device_id)
            }
            _ => None,
        }
    }

    fn rate_changed(&self) -> String {
        let direction = if matches!(self.node.kind, NodeKind::DeviceOutput { .. }) {
            "Sortie audio"
        } else {
            "Entrée audio"
        };
        format!(
            "{direction} : la fréquence de {} a changé.",
            self.node.label
        )
    }
}

pub(super) enum RecoveryRequest {
    Capture(usize, EndpointSpec),
    Output(usize, EndpointSpec),
    RetireCapture(Box<Capture>),
    RetireOutput(AudioOutput),
}

pub(super) enum Attempt<T> {
    Open(T),
    Retry,
    RestartRequired(String),
}

pub(super) enum RecoveryReply {
    Capture(usize, Attempt<Box<Capture>>),
    Output(usize, Attempt<AudioOutput>),
}

pub(super) struct Recovery {
    sender: Option<mpsc::Sender<RecoveryRequest>>,
    receiver: Option<mpsc::Receiver<RecoveryReply>>,
    cancelled: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Recovery {
    pub(super) fn start(sample_rate: u32, browser: BrowserCapture) -> Result<Self, String> {
        let (sender, requests) = mpsc::channel();
        let (replies, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker = thread::Builder::new()
            .name("patchbay-recovery".into())
            .spawn(move || {
                for request in requests {
                    if worker_cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let reply = match request {
                        RecoveryRequest::RetireCapture(capture) => {
                            drop(capture);
                            continue;
                        }
                        RecoveryRequest::RetireOutput(stream) => {
                            drop(stream);
                            continue;
                        }
                        RecoveryRequest::Capture(index, endpoint) => RecoveryReply::Capture(
                            index,
                            reopen(
                                &endpoint,
                                audio::device_rate,
                                || {
                                    audio::open_input(&endpoint.node.kind, sample_rate, &browser)
                                        .map(|input| Box::new(Capture::new(input, sample_rate)))
                                },
                                |capture| (capture.stream.sample_rate(), capture.stream.channels()),
                            ),
                        ),
                        RecoveryRequest::Output(index, endpoint) => {
                            let attempt = reopen(
                                &endpoint,
                                audio::device_rate,
                                || {
                                    audio::open_output(
                                        endpoint.device_id().expect("output device ID"),
                                        sample_rate,
                                    )
                                },
                                |output| (output.sample_rate(), output.channels()),
                            );
                            if let Attempt::Open(output) = &attempt {
                                prime_output(output);
                            }
                            RecoveryReply::Output(index, attempt)
                        }
                    };
                    if worker_cancelled.load(Ordering::Acquire) || replies.send(reply).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| format!("Impossible de démarrer la reprise audio ({error})."))?;
        Ok(Self {
            sender: Some(sender),
            receiver: Some(receiver),
            cancelled,
            worker: Some(worker),
        })
    }

    pub(super) fn send(&self, request: RecoveryRequest) -> bool {
        self.sender
            .as_ref()
            .expect("render plan still owns its request channel")
            .send(request)
            .is_ok()
    }

    pub(super) fn try_recv(&self) -> Option<RecoveryReply> {
        self.receiver
            .as_ref()
            .expect("render plan still owns its reply channel")
            .try_recv()
            .ok()
    }
}

impl Drop for Recovery {
    fn drop(&mut self) {
        // The mix loop has ended. Cancel queued work and wait for any in-flight open
        // and its teardown to finish before a new graph can take over the devices.
        self.cancelled.store(true, Ordering::Release);
        drop(self.receiver.take());
        drop(self.sender.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn reopen<T>(
    endpoint: &EndpointSpec,
    device_rate: impl FnOnce(&str) -> Option<u32>,
    open: impl FnOnce() -> Result<T, String>,
    shape: impl FnOnce(&T) -> (u32, usize),
) -> Attempt<T> {
    let device_id = endpoint.device_id();
    if let Some(id) = device_id {
        match device_rate(id) {
            Some(rate) if rate != endpoint.sample_rate => {
                return Attempt::RestartRequired(endpoint.rate_changed());
            }
            None if cfg!(target_os = "macos") => return Attempt::Retry,
            _ => {}
        }
    }
    let Ok(stream) = open() else {
        return Attempt::Retry;
    };
    let (rate, channels) = shape(&stream);
    // Check again after opening: the HAL can change while a stream is being recreated.
    if device_id.is_some() && rate != endpoint.sample_rate {
        return Attempt::RestartRequired(endpoint.rate_changed());
    }
    if channels != endpoint.channels {
        return Attempt::RestartRequired(format!(
            "Le nombre de canaux de {} a changé.",
            endpoint.node.label
        ));
    }
    Attempt::Open(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_refuses_a_changed_device_rate_or_channel_shape() {
        let endpoint = EndpointSpec {
            node: GraphNode {
                id: "device".into(),
                label: "Micro USB".into(),
                x: 0.0,
                y: 0.0,
                kind: NodeKind::DeviceInput {
                    device_id: "device".into(),
                },
            },
            sample_rate: 48_000,
            channels: 2,
        };
        assert!(matches!(
            reopen::<()>(&endpoint, |_| Some(44_100), || panic!("would open at an unexpected rate"), |_| unreachable!()),
            Attempt::RestartRequired(reason) if reason.contains("la fréquence de Micro USB a changé")
        ));
        assert!(matches!(
            reopen(&endpoint, |_| Some(48_000), || Ok((44_100, 2)), |shape| *shape),
            Attempt::RestartRequired(reason) if reason.contains("la fréquence de Micro USB a changé")
        ));
        assert!(matches!(
            reopen(&endpoint, |_| Some(48_000), || Ok((48_000, 1)), |shape| *shape),
            Attempt::RestartRequired(reason) if reason == "Le nombre de canaux de Micro USB a changé."
        ));
        assert!(matches!(
            reopen(
                &endpoint,
                |_| Some(48_000),
                || Ok((48_000, 2)),
                |shape| *shape
            ),
            Attempt::Open((48_000, 2))
        ));
    }

    #[test]
    fn shutdown_closes_channels_and_waits_for_the_worker() {
        let (sender, requests) = mpsc::channel();
        let (replies, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let worker = thread::spawn(move || {
            assert!(requests.recv().is_err());
            assert!(worker_cancelled.load(Ordering::Acquire));
            assert!(
                replies
                    .send(RecoveryReply::Output(0, Attempt::Retry))
                    .is_err()
            );
            worker_finished.store(true, Ordering::Release);
        });
        drop(Recovery {
            sender: Some(sender),
            receiver: Some(receiver),
            cancelled,
            worker: Some(worker),
        });
        assert!(finished.load(Ordering::Acquire));
    }
}

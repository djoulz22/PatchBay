use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::audio::{self, AudioInput, AudioOutput};
use crate::browser_audio::BrowserCapture;
use crate::graph::{
    ChannelLevel, GraphNode, MAX_CHANNELS, NodeKind, Project, Route, canonical_device_id,
    master_pan, master_side,
};
use crate::realtime;

mod recovery;
use recovery::{Attempt, EndpointSpec, Recovery, RecoveryReply, RecoveryRequest};

const BLOCKS_PER_SECOND: usize = 200;
const BLOCK_TIME: Duration = Duration::from_millis(1_000 / BLOCKS_PER_SECOND as u64);
const MIX_COMPUTATION_TIME: Duration = Duration::from_micros(1_500);
const HEALTH_INTERVAL: Duration = Duration::from_millis(250);
const METER_INTERVAL: Duration = Duration::from_millis(50);
const RECOVERY_INTERVAL: Duration = Duration::from_secs(1);
const OUTPUT_LOSS_TIMEOUT: Duration = Duration::from_secs(10);
const TONE_LEVEL: f32 = 0.125;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub running: bool,
    pub realtime: bool,
    pub error: Option<String>,
    pub offline: Vec<String>,
    pub meters: BTreeMap<String, Meter>,
    pub underruns: u64,
    pub dropped_frames: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meter {
    pub channels: Vec<f32>,
    pub clipping: bool,
    pub clipped_channels: Vec<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master: Option<MasterMeter>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterMeter {
    pub channels: [f32; 2],
    pub clipped_channels: [bool; 2],
}

impl Meter {
    fn new(channels: usize) -> Self {
        Self {
            channels: vec![0.0; channels],
            clipping: false,
            clipped_channels: vec![false; channels],
            master: None,
        }
    }

    fn observe_master(&mut self, frame: [f32; 2]) {
        let master = self.master.get_or_insert_default();
        let mut overload = false;
        for ((peak, clipped), sample) in master
            .channels
            .iter_mut()
            .zip(&mut master.clipped_channels)
            .zip(frame)
        {
            let level = sample.abs();
            let channel_overload = level > 1.0;
            *clipped |= channel_overload;
            overload |= channel_overload;
            *peak = peak.max(level.min(1.0));
        }
        self.clipping |= overload;
    }

    fn observe(&mut self, frame: &[f32]) {
        for ((peak, clipped), sample) in self
            .channels
            .iter_mut()
            .zip(&mut self.clipped_channels)
            .zip(frame)
        {
            let level = sample.abs();
            let overload = level > 1.0;
            self.clipping |= overload;
            *clipped |= overload;
            *peak = peak.max(level.min(1.0));
        }
    }

    fn report(&mut self, id: &str, meters: &mut BTreeMap<String, Self>) {
        if let Some(previous) = meters.get_mut(id) {
            previous.clone_from(self);
        } else {
            meters.insert(id.to_owned(), self.clone());
        }
        self.clipping = false;
        self.channels.fill(0.0);
        self.clipped_channels.fill(false);
        if let Some(master) = self.master.as_mut() {
            *master = MasterMeter::default();
        }
    }
}

pub struct Runtime {
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<EngineStatus>>,
    pending_gains: Arc<Mutex<Option<MixLevels>>>,
    topology: Arc<Project>,
    virtual_devices: HashSet<String>,
}

impl Runtime {
    pub fn start(project: Project, browser: BrowserCapture) -> Result<Self, String> {
        project.validate()?;
        if project.routes.is_empty() {
            return Err("Trace au moins une liaison avant de démarrer le routage.".into());
        }

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(EngineStatus::default()));
        let pending_gains = Arc::new(Mutex::new(None));
        let topology = Arc::new(project);
        let worker_stop = Arc::clone(&stop);
        let worker_status = Arc::clone(&status);
        let worker_pending_gains = Arc::clone(&pending_gains);
        let worker_project = Arc::clone(&topology);
        let worker = thread::Builder::new()
            .name("patchbay-audio".into())
            .spawn(move || {
                let (mut plan, virtual_devices) = match RenderPlan::open(&worker_project, &browser)
                {
                    Ok(plan) => plan,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let recovery = match Recovery::start(plan.sample_rate, browser) {
                    Ok(recovery) => recovery,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let promoted = realtime::promote_current_thread(BLOCK_TIME, MIX_COMPUTATION_TIME);
                {
                    let mut status = worker_status.lock().expect("audio status lock");
                    status.realtime = cfg!(target_os = "macos") && promoted.is_ok();
                    status.running = true;
                }
                if ready_tx.send(Ok(virtual_devices)).is_err() {
                    return;
                }
                let result = plan.run(
                    &worker_stop,
                    &worker_status,
                    &worker_pending_gains,
                    &recovery,
                );
                let mut status = worker_status.lock().expect("audio status lock");
                status.running = false;
                status.realtime = false;
                status.error = result.err();
                status.meters.clear();
                status.offline.clear();
            })
            .map_err(|error| format!("Démarrage du moteur audio impossible. {error}"))?;
        let mut runtime = Self {
            worker: Some(worker),
            stop,
            status,
            pending_gains,
            topology,
            virtual_devices: HashSet::new(),
        };
        runtime.virtual_devices = ready_rx
            .recv()
            .map_err(|_| "Le moteur audio s'est interrompu au démarrage.".to_owned())??;
        Ok(runtime)
    }

    pub fn status(&self) -> EngineStatus {
        let mut status = self.status.lock().expect("audio status lock").clone();
        if status.running && self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            status.running = false;
            status.realtime = false;
            status.error = Some("Le moteur audio s'est interrompu.".into());
            status.meters.clear();
            status.offline.clear();
        }
        status
    }

    pub fn update_mix(&self, project: &Project) -> Result<(), String> {
        if !self.status.lock().expect("audio status lock").running {
            return Err("Le routage est arrêté.".into());
        }
        let gains = updated_gains(&self.topology, project, &self.virtual_devices)?;
        *self.pending_gains.lock().expect("audio gains lock") = Some(gains);
        Ok(())
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            self.stop.store(true, Ordering::Release);
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct MixLevels {
    routes: Vec<f32>,
    output_channels: HashMap<String, OutputMix>,
}

fn updated_gains(
    current: &Project,
    requested: &Project,
    virtual_devices: &HashSet<String>,
) -> Result<MixLevels, String> {
    requested.validate()?;
    let restart = || "Le nouveau câblage sera utilisé au redémarrage du routage.".to_owned();
    if requested.sample_rate != current.sample_rate
        || requested.routes.len() != current.routes.len()
    {
        return Err(restart());
    }
    let kinds: HashMap<_, _> = requested
        .nodes
        .iter()
        .map(|node| (&node.id, &node.kind))
        .collect();
    let mut output_channels = HashMap::new();
    for connected in current.connected_nodes() {
        let updated = kinds.get(&connected.id).copied().ok_or_else(restart)?;
        match (&connected.kind, updated) {
            (
                NodeKind::OutputChannels { channels, .. },
                NodeKind::OutputChannels {
                    channels: new_channels,
                    levels,
                    stereo_pairs,
                    master,
                },
            ) if channels == new_channels => {
                output_channels.insert(
                    connected.id.clone(),
                    OutputMix::new(*channels, levels, stereo_pairs, master.as_ref()),
                );
            }
            (original, edited) if original == edited => {}
            _ => return Err(restart()),
        }
    }
    let routes: HashMap<_, _> = requested
        .routes
        .iter()
        .map(|route| (&route.id, route))
        .collect();
    let routes = current
        .routes
        .iter()
        .map(|route| {
            routes
                .get(&route.id)
                .filter(|edit| edit.ports() == route.ports())
                .map(|edit| route_gain(edit))
                .ok_or_else(restart)
        })
        .collect::<Result<_, _>>()?;
    if requested
        .routes
        .iter()
        .any(|route| master_side(route.source_channel).is_some())
    {
        requested.reject_virtual_feedback(&virtual_devices.iter().map(String::as_str).collect())?;
    }
    Ok(MixLevels {
        routes,
        output_channels,
    })
}

struct RenderPlan {
    sample_rate: u32,
    sources: Vec<Source>,
    destinations: Vec<Destination>,
    mixer: Mixer,
    dropped: u64,
}

impl RenderPlan {
    fn open(
        project: &Project,
        browser: &BrowserCapture,
    ) -> Result<(Self, HashSet<String>), String> {
        let virtual_devices: HashSet<_> = if project.connected_nodes().any(|node| {
            matches!(
                node.kind,
                NodeKind::DeviceInput { .. } | NodeKind::DeviceOutput { .. }
            )
        }) {
            let (inputs, outputs) = audio::devices()?;
            inputs
                .into_iter()
                .chain(outputs)
                .filter(|device| device.is_virtual)
                .map(|device| device.id)
                .collect()
        } else {
            HashSet::new()
        };
        project.reject_virtual_feedback(&virtual_devices.iter().map(String::as_str).collect())?;

        let mut destinations = Vec::new();
        let mut destination_devices = HashSet::new();
        // Register our output streams with Core Audio before the system tap resolves its exclusion list.
        for node in project.connected_nodes() {
            if let NodeKind::DeviceOutput { device_id } = &node.kind {
                if !destination_devices.insert(canonical_device_id(device_id)) {
                    return Err(format!(
                        "La sortie « {} » est déjà utilisée dans le graphe.",
                        node.label
                    ));
                }
                destinations.push(Destination::open(node, device_id, project.sample_rate)?);
            }
        }
        let mut sources = Vec::new();
        for node in project.connected_nodes() {
            if !node.kind.is_stage() && !matches!(node.kind, NodeKind::DeviceOutput { .. }) {
                sources.push(Source::open(node, project.sample_rate, browser)?);
            }
        }
        let mixer = Mixer::compile(
            project,
            &sources,
            destinations.iter().map(|device| {
                (
                    device.id.as_str(),
                    device.label.as_str(),
                    device.frame.len(),
                )
            }),
        )?;
        Ok((
            Self {
                sample_rate: project.sample_rate,
                sources,
                destinations,
                mixer,
                dropped: 0,
            },
            virtual_devices,
        ))
    }

    fn run(
        &mut self,
        stop: &AtomicBool,
        status: &Mutex<EngineStatus>,
        pending_gains: &Mutex<Option<MixLevels>>,
        recovery: &Recovery,
    ) -> Result<(), String> {
        for destination in &mut self.destinations {
            destination.prime();
        }
        let mut clock = BlockClock::new(self.sample_rate);
        let mut next_tick = Instant::now();
        let mut next_health = next_tick + HEALTH_INTERVAL;
        let mut next_meter = next_tick + METER_INTERVAL;
        while !stop.load(Ordering::Acquire) {
            let now = Instant::now();
            if now < next_tick {
                thread::park_timeout(next_tick - now);
                continue;
            }
            if now - next_tick >= BLOCK_TIME * 5 {
                let skipped_frames = (now - next_tick).as_secs_f64() * f64::from(self.sample_rate);
                self.dropped = self.dropped.saturating_add(skipped_frames as u64);
                next_tick = now;
            }
            next_tick += BLOCK_TIME;
            self.apply_recovery_replies(now, recovery);
            if let Some(gains) = pending_gains.lock().expect("audio gains lock").take() {
                self.mixer.set_gains(gains, self.sample_rate / 100);
            }
            self.render(clock.next_frames());
            if now >= next_health {
                self.check_streams(now, recovery);
                if all_outputs_lost(
                    self.destinations
                        .iter()
                        .map(|output| output.offline.as_ref().map(|offline| offline.since)),
                    now,
                ) {
                    return Err("Toutes les sorties du graphe sont indisponibles.".into());
                }
                next_health = now + HEALTH_INTERVAL;
            }
            if now >= next_meter {
                self.report(status);
                next_meter = now + METER_INTERVAL;
            }
        }
        self.report(status);
        Ok(())
    }

    fn render(&mut self, frames: usize) {
        for source in &mut self.sources {
            source.begin_block();
        }
        for destination in &mut self.destinations {
            destination.begin_block(self.sample_rate);
        }
        for _ in 0..frames {
            for source in &mut self.sources {
                source.read_frame();
            }
            for destination in &mut self.destinations {
                destination.frame.fill(0.0);
            }
            self.mixer
                .mix_frame(&self.sources, |device, channel, sample| {
                    self.destinations[device].frame[channel] += sample;
                });
            for destination in &mut self.destinations {
                destination.write_frame();
            }
        }
    }

    fn check_streams(&mut self, now: Instant, recovery: &Recovery) {
        for (index, source) in self.sources.iter_mut().enumerate() {
            if let SourceSignal::Capture(capture) = &source.signal
                && let Some(error) = capture.stream.error()
            {
                let offline = Offline::new(now, capture.stream.sample_rate(), error);
                let SourceSignal::Capture(capture) =
                    std::mem::replace(&mut source.signal, SourceSignal::Offline(offline))
                else {
                    unreachable!("source fault can only come from a capture");
                };
                recovery.send(RecoveryRequest::RetireCapture(capture));
            }
            if let SourceSignal::Offline(offline) = &mut source.signal
                && offline.recovery_due(now)
            {
                let endpoint = EndpointSpec::new(&source.node, offline, source.frame.len());
                offline.request(now, recovery, RecoveryRequest::Capture(index, endpoint));
            }
        }
        for (index, destination) in self.destinations.iter_mut().enumerate() {
            if let Some(stream) = &destination.stream
                && let Some(error) = stream.error()
            {
                destination.offline = Some(Offline::new(now, stream.sample_rate(), error));
                if let Some(stream) = destination.stream.take() {
                    recovery.send(RecoveryRequest::RetireOutput(stream));
                }
            }
            if let Some(offline) = &mut destination.offline
                && offline.recovery_due(now)
            {
                let endpoint =
                    EndpointSpec::new(&destination.node, offline, destination.frame.len());
                offline.request(now, recovery, RecoveryRequest::Output(index, endpoint));
            }
        }
    }

    fn apply_recovery_replies(&mut self, now: Instant, recovery: &Recovery) {
        while let Some(reply) = recovery.try_recv() {
            match reply {
                RecoveryReply::Capture(index, attempt) => {
                    let source = &mut self.sources[index];
                    if let SourceSignal::Offline(offline) = &mut source.signal
                        && let Some(capture) = offline.finish(now, attempt)
                    {
                        source.signal = SourceSignal::Capture(capture);
                    }
                }
                RecoveryReply::Output(index, attempt) => {
                    let destination = &mut self.destinations[index];
                    if let Some(offline) = &mut destination.offline
                        && let Some(stream) = offline.finish(now, attempt)
                    {
                        destination.restore(stream);
                    }
                }
            }
        }
    }

    fn report(&mut self, status: &Mutex<EngineStatus>) {
        let mut status = status.lock().expect("audio status lock");
        status.dropped_frames = self.dropped;
        status.underruns = 0;
        for source in &mut self.sources {
            source.meter.report(&source.id, &mut status.meters);
            if let SourceSignal::Capture(capture) = &source.signal {
                status.dropped_frames = status
                    .dropped_frames
                    .saturating_add(capture.dropped)
                    .saturating_add(capture.stream.dropped_frames());
            }
        }
        for stage in &mut self.mixer.stages {
            stage.meter.report(&stage.id, &mut status.meters);
        }
        for destination in &mut self.destinations {
            destination
                .meter
                .report(&destination.id, &mut status.meters);
            if let Some(stream) = &destination.stream {
                status.underruns = status.underruns.saturating_add(
                    stream
                        .underruns()
                        .saturating_sub(destination.initial_underruns),
                );
                status.dropped_frames = status.dropped_frames.saturating_add(destination.dropped);
            }
        }
        let offline_sources = self.sources.iter().filter_map(|source| {
            if let SourceSignal::Offline(offline) = &source.signal {
                Some((&source.id, offline))
            } else {
                None
            }
        });
        let offline_outputs = self.destinations.iter().filter_map(|destination| {
            destination
                .offline
                .as_ref()
                .map(|offline| (&destination.id, offline))
        });
        status.offline.clear();
        let mut latest: Option<&Offline> = None;
        for (id, offline) in offline_sources.chain(offline_outputs) {
            status.offline.push(id.clone());
            if latest.is_none_or(|previous| previous.since <= offline.since) {
                latest = Some(offline);
            }
        }
        status.offline.sort_unstable();
        status.error = latest.map(Offline::message);
    }
}

fn all_outputs_lost(outputs: impl IntoIterator<Item = Option<Instant>>, now: Instant) -> bool {
    let mut has_outputs = false;
    for since in outputs {
        has_outputs = true;
        if since.is_none_or(|since| now.duration_since(since) < OUTPUT_LOSS_TIMEOUT) {
            return false;
        }
    }
    has_outputs
}

struct Offline {
    since: Instant,
    last_attempt: Instant,
    sample_rate: u32,
    reason: String,
    restart_required: bool,
    in_flight: bool,
}

impl Offline {
    fn new(now: Instant, sample_rate: u32, error: audio::StreamFault) -> Self {
        Self {
            since: now,
            last_attempt: now,
            sample_rate,
            reason: error.message,
            restart_required: error.restart_required,
            in_flight: false,
        }
    }

    fn recovery_due(&self, now: Instant) -> bool {
        !self.restart_required
            && !self.in_flight
            && now.duration_since(self.last_attempt) >= RECOVERY_INTERVAL
    }

    fn request(&mut self, now: Instant, recovery: &Recovery, request: RecoveryRequest) {
        self.last_attempt = now;
        self.in_flight = recovery.send(request);
    }

    fn finish<T>(&mut self, now: Instant, attempt: Attempt<T>) -> Option<T> {
        self.in_flight = false;
        // A queued request may have waited behind a slow native open. Start the next
        // interval here so that such a delay cannot cause back-to-back attempts.
        self.last_attempt = now;
        match attempt {
            Attempt::Open(stream) => Some(stream),
            Attempt::Retry => None,
            Attempt::RestartRequired(reason) => {
                self.reason = reason;
                self.restart_required = true;
                None
            }
        }
    }

    fn message(&self) -> String {
        let action = if self.restart_required {
            "Redémarre le routage."
        } else {
            "Reprise automatique en attente."
        };
        format!("{}. {action}", self.reason.trim_end_matches('.'))
    }
}

fn check_channel(channel: usize, channels: usize, label: &str) -> Result<(), String> {
    if channel >= channels {
        return Err(format!(
            "Canal {} indisponible sur « {label} ».",
            channel + 1
        ));
    }
    Ok(())
}

struct BlockClock {
    sample_rate: usize,
    remainder: usize,
}

impl BlockClock {
    fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate as usize,
            remainder: 0,
        }
    }

    fn next_frames(&mut self) -> usize {
        self.remainder += self.sample_rate;
        let frames = self.remainder / BLOCKS_PER_SECOND;
        self.remainder %= BLOCKS_PER_SECOND;
        frames
    }
}

fn route_gain(route: &Route) -> f32 {
    if route.muted { 0.0 } else { route.gain }
}

fn channel_gain(level: Option<&ChannelLevel>) -> f32 {
    level.map_or(1.0, |level| if level.muted { 0.0 } else { level.gain })
}

struct OutputMix {
    gains: Vec<f32>,
    master_gain: f32,
    master_pan: Vec<[f32; 2]>,
}

impl OutputMix {
    fn new(
        channels: usize,
        levels: &[ChannelLevel],
        stereo_pairs: &[[usize; 2]],
        master: Option<&ChannelLevel>,
    ) -> Self {
        Self {
            gains: (0..channels)
                .map(|index| channel_gain(levels.get(index)))
                .collect(),
            master_gain: channel_gain(master),
            master_pan: master_pan(channels, stereo_pairs),
        }
    }
}

struct Stage {
    id: String,
    inputs: Vec<MixRoute>,
    frame: Vec<f32>,
    meter: Meter,
    output: Option<OutputStage>,
}

impl Stage {
    fn new(node: &GraphNode) -> Self {
        let (channels, output) = match &node.kind {
            NodeKind::OutputChannels {
                channels,
                levels,
                stereo_pairs,
                master,
            } => (
                *channels,
                Some(OutputStage::new(OutputMix::new(
                    *channels,
                    levels,
                    stereo_pairs,
                    master.as_ref(),
                ))),
            ),
            NodeKind::InternalBus { channels } => (*channels, None),
            _ => unreachable!("non-stage in stage order"),
        };
        Self {
            id: node.id.clone(),
            inputs: Vec::new(),
            frame: vec![0.0; channels],
            meter: Meter::new(channels),
            output,
        }
    }

    fn sample(&self, channel: usize) -> f32 {
        match master_side(channel) {
            Some(side) => {
                self.output
                    .as_ref()
                    .expect("validated master port")
                    .master_frame[side]
            }
            None => self.frame[channel],
        }
    }
}

struct OutputStage {
    gains: Vec<GainRamp>,
    master_gain: GainRamp,
    master_pan: Vec<[f32; 2]>,
    master_frame: [f32; 2],
}

impl OutputStage {
    fn new(mix: OutputMix) -> Self {
        Self {
            gains: mix.gains.into_iter().map(GainRamp::new).collect(),
            master_gain: GainRamp::new(mix.master_gain),
            master_pan: mix.master_pan,
            master_frame: [0.0; 2],
        }
    }

    fn finish_frame(&mut self, frame: &mut [f32], meter: &mut Meter) {
        let mut master = [0.0; 2];
        for ((sample, gain), [left, right]) in
            frame.iter_mut().zip(&mut self.gains).zip(&self.master_pan)
        {
            *sample = gain.apply(*sample);
            master[0] += *sample * left;
            master[1] += *sample * right;
        }
        meter.observe(frame);
        for sample in frame {
            *sample = sample.clamp(-1.0, 1.0);
        }

        // Advance the stereo fader once per frame so both sides use the same ramp.
        let gain = self.master_gain.apply(1.0);
        let master = master.map(|sample| sample * gain);
        meter.observe_master(master);
        self.master_frame = master.map(|sample| sample.clamp(-1.0, 1.0));
    }
}

struct Mixer {
    stages: Vec<Stage>,
    to_devices: Vec<DeviceRoute>,
    // Keep the order of the original cables for live gain and mute updates.
    gains: Vec<GainRamp>,
}

impl Mixer {
    fn compile<'a>(
        project: &Project,
        sources: &[Source],
        devices: impl Iterator<Item = (&'a str, &'a str, usize)>,
    ) -> Result<Self, String> {
        let ordered_stages = project.ordered_stages()?;
        let stages_by_id: HashMap<_, _> = ordered_stages
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        let sources_by_id: HashMap<_, _> = sources
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        let devices_by_id: HashMap<_, _> = devices
            .enumerate()
            .map(|(index, (id, label, channels))| (id, (index, label, channels)))
            .collect();

        let mut mixer = Self {
            stages: ordered_stages.into_iter().map(Stage::new).collect(),
            to_devices: Vec::new(),
            gains: project
                .routes
                .iter()
                .map(route_gain)
                .map(GainRamp::new)
                .collect(),
        };
        for (gain_index, route) in project.routes.iter().enumerate() {
            let source = if let Some(&index) = stages_by_id.get(route.source.as_str()) {
                FrameSource::Stage(index)
            } else {
                let index = sources_by_id[route.source.as_str()];
                let input = &sources[index];
                check_channel(route.source_channel, input.frame.len(), &input.label)?;
                FrameSource::Input(index)
            };
            let input = MixRoute {
                source,
                source_channel: route.source_channel,
                target_channel: route.target_channel,
                gain_index,
            };
            if let Some(&index) = stages_by_id.get(route.target.as_str()) {
                mixer.stages[index].inputs.push(input);
            } else {
                let (index, label, channels) = devices_by_id[route.target.as_str()];
                check_channel(route.target_channel, channels, label)?;
                mixer.to_devices.push(DeviceRoute { input, index });
            }
        }
        Ok(mixer)
    }

    fn set_gains(&mut self, mut gains: MixLevels, frames: u32) {
        for (ramp, gain) in self.gains.iter_mut().zip(gains.routes) {
            ramp.set_gain(gain, frames);
        }
        for stage in &mut self.stages {
            if let Some(output) = &mut stage.output
                && let Some(mix) = gains.output_channels.remove(&stage.id)
            {
                for (ramp, gain) in output.gains.iter_mut().zip(mix.gains) {
                    ramp.set_gain(gain, frames);
                }
                output.master_gain.set_gain(mix.master_gain, frames);
                output.master_pan = mix.master_pan;
            }
        }
    }

    fn mix_frame(&mut self, sources: &[Source], mut send: impl FnMut(usize, usize, f32)) {
        for index in 0..self.stages.len() {
            // The compiled order puts every contributing stage before the stage that reads it.
            let (upstream, remaining) = self.stages.split_at_mut(index);
            let stage = &mut remaining[0];
            stage.frame.fill(0.0);
            for route in &stage.inputs {
                stage.frame[route.target_channel] +=
                    route.apply(sources, upstream, &mut self.gains);
            }
            if let Some(output) = &mut stage.output {
                output.finish_frame(&mut stage.frame, &mut stage.meter);
            } else {
                stage.meter.observe(&stage.frame);
            }
        }
        for route in &self.to_devices {
            let sample = route.input.apply(sources, &self.stages, &mut self.gains);
            send(route.index, route.input.target_channel, sample);
        }
    }
}

#[derive(Clone, Copy)]
enum FrameSource {
    Input(usize),
    Stage(usize),
}

struct MixRoute {
    source: FrameSource,
    source_channel: usize,
    target_channel: usize,
    gain_index: usize,
}

impl MixRoute {
    fn apply(&self, sources: &[Source], stages: &[Stage], gains: &mut [GainRamp]) -> f32 {
        let sample = match self.source {
            FrameSource::Input(index) => sources[index].frame[self.source_channel],
            FrameSource::Stage(index) => stages[index].sample(self.source_channel),
        };
        gains[self.gain_index].apply(sample)
    }
}

struct DeviceRoute {
    input: MixRoute,
    index: usize,
}

struct GainRamp {
    gain: f32,
    target_gain: f32,
    increment: f32,
    ramp_frames: u32,
}

impl GainRamp {
    fn new(gain: f32) -> Self {
        Self {
            gain,
            target_gain: gain,
            increment: 0.0,
            ramp_frames: 0,
        }
    }

    fn set_gain(&mut self, gain: f32, frames: u32) {
        if self.target_gain != gain {
            self.target_gain = gain;
            self.increment = (gain - self.gain) / frames as f32;
            self.ramp_frames = frames;
        }
    }

    fn apply(&mut self, input: f32) -> f32 {
        if self.ramp_frames > 0 {
            self.ramp_frames -= 1;
            self.gain += self.increment;
            if self.ramp_frames == 0 {
                self.gain = self.target_gain;
            }
        }
        input * self.gain
    }
}

struct Source {
    id: String,
    label: String,
    node: GraphNode,
    signal: SourceSignal,
    frame: Vec<f32>,
    meter: Meter,
}

enum SourceSignal {
    Tone { phase: f64, increment: f64 },
    Capture(Box<Capture>),
    Offline(Offline),
}

impl Source {
    fn open(node: &GraphNode, sample_rate: u32, browser: &BrowserCapture) -> Result<Self, String> {
        let (signal, channels) = match &node.kind {
            NodeKind::Tone { frequency_hz } => (
                SourceSignal::Tone {
                    phase: 0.0,
                    increment: frequency_hz / f64::from(sample_rate),
                },
                1,
            ),
            kind => {
                let input = audio::open_input(kind, sample_rate, browser)
                    .map_err(|error| format!("{} : {error}", node.label))?;
                let channels = input.channels();
                (
                    SourceSignal::Capture(Box::new(Capture::new(input, sample_rate))),
                    channels,
                )
            }
        };
        Ok(Self {
            id: node.id.clone(),
            label: node.label.clone(),
            node: node.clone(),
            signal,
            frame: vec![0.0; channels],
            meter: Meter::new(channels),
        })
    }

    fn begin_block(&mut self) {
        if let SourceSignal::Capture(capture) = &mut self.signal {
            capture.begin_block();
        }
    }

    fn read_frame(&mut self) {
        match &mut self.signal {
            SourceSignal::Tone { phase, increment } => {
                self.frame[0] = (*phase * std::f64::consts::TAU).sin() as f32 * TONE_LEVEL;
                *phase += *increment;
                if *phase >= 1.0 {
                    *phase -= 1.0;
                }
            }
            SourceSignal::Capture(capture) => capture.read(&mut self.frame),
            SourceSignal::Offline(_) => self.frame.fill(0.0),
        }
        for sample in &mut self.frame {
            if !sample.is_finite() {
                *sample = 0.0;
            }
        }
        self.meter.observe(&self.frame);
    }
}

#[derive(Default)]
struct ClockRecovery {
    correction: f64,
}

impl ClockRecovery {
    fn update(&mut self, queued: usize, target: usize) -> f64 {
        let relative_error = (queued as f64 - target as f64) / target as f64;
        let desired = (relative_error * 0.004).clamp(-0.006, 0.006);
        self.correction += (desired - self.correction) * 0.02;
        1.0 + self.correction
    }
}

struct Capture {
    stream: AudioInput,
    previous: Vec<f32>,
    next: Vec<f32>,
    phase: f64,
    nominal_ratio: f64,
    step: f64,
    prefill: usize,
    primed: bool,
    clock: ClockRecovery,
    dropped: u64,
}

impl Capture {
    fn new(stream: AudioInput, engine_rate: u32) -> Self {
        let channels = stream.channels();
        let ratio = f64::from(stream.sample_rate()) / f64::from(engine_rate);
        // The browser sends short PCM batches. Allow for delivery jitter before playback.
        let prefill = if stream.is_browser() {
            1_600
        } else {
            (stream.sample_rate() as usize / 65).clamp(32, 1_280)
        };
        Self {
            stream,
            previous: vec![0.0; channels],
            next: vec![0.0; channels],
            phase: 0.0,
            nominal_ratio: ratio,
            step: ratio,
            prefill,
            primed: false,
            clock: ClockRecovery::default(),
            dropped: 0,
        }
    }

    fn begin_block(&mut self) {
        let mut queued = self.stream.available_frames();
        let high_watermark = (self.prefill * 4).min(3_600);
        if queued > high_watermark {
            while queued > self.prefill * 2 && self.stream.pop_frame(&mut self.next) {
                queued -= 1;
                self.dropped += 1;
            }
            self.primed = false;
        }
        self.step = self.nominal_ratio * self.clock.update(queued, self.prefill);
    }

    fn read(&mut self, frame: &mut [f32]) {
        if !self.primed {
            if self.stream.available_frames() < self.prefill
                || !self.stream.pop_frame(&mut self.previous)
                || !self.stream.pop_frame(&mut self.next)
            {
                frame.fill(0.0);
                return;
            }
            self.phase = 0.0;
            self.primed = true;
        }
        while self.phase >= 1.0 {
            self.previous.copy_from_slice(&self.next);
            if !self.stream.pop_frame(&mut self.next) {
                self.primed = false;
                frame.fill(0.0);
                return;
            }
            self.phase -= 1.0;
        }
        interpolate(&self.previous, &self.next, self.phase as f32, frame);
        self.phase += self.step;
    }
}

struct Destination {
    id: String,
    label: String,
    node: GraphNode,
    stream: Option<AudioOutput>,
    offline: Option<Offline>,
    frame: Vec<f32>,
    previous: Vec<f32>,
    converted: Vec<f32>,
    phase: f64,
    step: f64,
    clock: ClockRecovery,
    initial_underruns: u64,
    dropped: u64,
    meter: Meter,
}

impl Destination {
    fn open(node: &GraphNode, device_id: &str, engine_rate: u32) -> Result<Self, String> {
        let stream = audio::open_output(device_id, engine_rate)
            .map_err(|error| format!("{} : {error}", node.label))?;
        let channels = stream.channels();
        let step = f64::from(engine_rate) / f64::from(stream.sample_rate());
        Ok(Self {
            id: node.id.clone(),
            label: node.label.clone(),
            node: node.clone(),
            stream: Some(stream),
            offline: None,
            frame: vec![0.0; channels],
            previous: vec![0.0; channels],
            converted: vec![0.0; channels],
            phase: 0.0,
            step,
            clock: ClockRecovery::default(),
            initial_underruns: 0,
            dropped: 0,
            meter: Meter::new(channels),
        })
    }

    fn prime(&mut self) {
        if let Some(stream) = &self.stream {
            prime_output(stream);
            self.initial_underruns = stream.underruns();
        }
    }

    fn restore(&mut self, stream: AudioOutput) {
        self.frame.fill(0.0);
        self.previous.fill(0.0);
        self.converted.fill(0.0);
        self.phase = 0.0;
        self.clock = ClockRecovery::default();
        self.dropped = 0;
        self.initial_underruns = stream.underruns();
        self.stream = Some(stream);
        self.offline = None;
    }

    fn begin_block(&mut self, engine_rate: u32) {
        if let Some(stream) = &self.stream {
            let sample_rate = stream.sample_rate();
            let target = (sample_rate as usize / 50).clamp(32, 1_250);
            self.step = f64::from(engine_rate) / f64::from(sample_rate)
                * self.clock.update(stream.queued_frames(), target);
        }
    }

    fn write_frame(&mut self) {
        self.meter.observe(&self.frame);
        let Some(stream) = &self.stream else {
            return;
        };
        for sample in &mut self.frame {
            *sample = sample.clamp(-1.0, 1.0);
        }
        while self.phase < 1.0 {
            interpolate(
                &self.previous,
                &self.frame,
                self.phase as f32,
                &mut self.converted,
            );
            if !stream.push_frame(&self.converted) {
                self.dropped += 1;
            }
            self.phase += self.step;
        }
        self.phase -= 1.0;
        self.previous.copy_from_slice(&self.frame);
    }
}

fn prime_output(stream: &AudioOutput) {
    let lead = (stream.sample_rate() as usize / 40).clamp(64, 1_600);
    let silence = [0.0; MAX_CHANNELS];
    for _ in 0..lead {
        if !stream.push_frame(&silence) {
            break;
        }
    }
}

fn interpolate(previous: &[f32], next: &[f32], fraction: f32, output: &mut [f32]) {
    for ((prior, following), sample) in previous.iter().zip(next).zip(output) {
        *sample = (following - prior).mul_add(fraction, *prior);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_outage(since: Instant, message: &str) -> Offline {
        Offline::new(
            since,
            48_000,
            audio::StreamFault {
                message: message.into(),
                restart_required: false,
            },
        )
    }

    #[test]
    fn recovery_waits_one_second_and_never_retries_a_pending_request() {
        let now = Instant::now();
        let mut outage = sample_outage(now, "Micro USB déconnecté");
        assert!(!outage.recovery_due(now + Duration::from_millis(999)));
        assert!(outage.recovery_due(now + RECOVERY_INTERVAL));
        outage.in_flight = true;
        assert!(!outage.recovery_due(now + RECOVERY_INTERVAL * 4));
        let completed = now + RECOVERY_INTERVAL * 5;
        assert!(outage.finish::<()>(completed, Attempt::Retry).is_none());
        assert!(!outage.recovery_due(completed + Duration::from_millis(999)));
        assert!(outage.recovery_due(completed + RECOVERY_INTERVAL));
        assert!(
            outage
                .finish::<()>(
                    completed,
                    Attempt::RestartRequired("La fréquence du micro a changé.".into())
                )
                .is_none()
        );
        assert!(!outage.recovery_due(completed + RECOVERY_INTERVAL * 4));
        assert_eq!(
            outage.message(),
            "La fréquence du micro a changé. Redémarre le routage."
        );
    }

    #[test]
    fn output_timeout_requires_a_continuous_outage_of_every_output() {
        let first = Instant::now();
        let last = first + Duration::from_secs(2);
        let deadline = last + OUTPUT_LOSS_TIMEOUT;
        assert!(!all_outputs_lost([], deadline));
        assert!(!all_outputs_lost([Some(first), None], deadline));
        assert!(!all_outputs_lost(
            [Some(first), Some(last)],
            deadline - Duration::from_millis(1)
        ));
        assert!(all_outputs_lost([Some(first), Some(last)], deadline));
    }

    #[test]
    fn an_offline_source_is_silent_and_preserves_other_mix_signals() {
        let project: Project = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "nodes": [
                { "id": "z", "label": "USB", "x": 0, "y": 0,
                    "kind": { "type": "tone", "frequencyHz": 1000 } },
                { "id": "a", "label": "Application", "x": 0, "y": 100,
                    "kind": { "type": "tone", "frequencyHz": 1000 } },
                { "id": "tone", "label": "Tonalité", "x": 0, "y": 200,
                    "kind": { "type": "tone", "frequencyHz": 1000 } },
                { "id": "bus", "label": "Bus", "x": 200, "y": 0,
                    "kind": { "type": "internalBus", "channels": 1 } }
            ],
            "routes": [
                { "id": "usb", "source": "z", "target": "bus", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "application", "source": "a", "target": "bus", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "keep", "source": "tone", "target": "bus", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 1.0, "muted": false }
            ]
        }))
        .unwrap();
        let (mut plan, _) = RenderPlan::open(&project, &BrowserCapture::default()).unwrap();
        let now = Instant::now();
        let usb = std::mem::replace(
            &mut plan.sources[0].signal,
            SourceSignal::Offline(sample_outage(now, "Micro USB déconnecté")),
        );
        let app = std::mem::replace(
            &mut plan.sources[1].signal,
            SourceSignal::Offline(sample_outage(
                now + RECOVERY_INTERVAL,
                "Capture interrompue",
            )),
        );
        plan.sources[0].frame.fill(1.0);
        plan.sources[1].frame.fill(1.0);
        plan.render(240);
        let status = Mutex::new(EngineStatus::default());
        plan.report(&status);
        {
            let status = status.lock().unwrap();
            assert_eq!(status.meters["z"].channels, [0.0]);
            assert_eq!(status.meters["a"].channels, [0.0]);
            assert_eq!(status.meters["bus"].channels, [TONE_LEVEL]);
            assert_eq!(status.offline, ["a", "z"]);
            assert_eq!(
                status.error.as_deref(),
                Some("Capture interrompue. Reprise automatique en attente.")
            );
        }
        plan.sources[0].signal = usb;
        plan.sources[1].signal = app;
        plan.report(&status);
        let status = status.into_inner().unwrap();
        assert!(status.offline.is_empty());
        assert!(status.error.is_none());
    }

    #[test]
    fn block_clock_has_no_rounding_drift_at_44100() {
        let mut clock = BlockClock::new(44_100);
        assert_eq!(clock.next_frames(), 220);
        assert_eq!(clock.next_frames(), 221);
        assert_eq!(
            (0..BLOCKS_PER_SECOND)
                .map(|_| clock.next_frames())
                .sum::<usize>(),
            44_100
        );
    }

    #[test]
    fn summing_and_ramping_report_clipping_before_saturation() {
        let mut first = GainRamp::new(1.0);
        let mut second = GainRamp::new(1.0);
        let sample = first.apply(0.75) + second.apply(0.75);
        assert_eq!(sample, 1.5);
        let mut meter = Meter::new(1);
        meter.observe(&[sample]);
        assert!(meter.clipping);
        assert_eq!(meter.channels, [1.0]);

        first.set_gain(0.0, 2);
        assert_eq!(first.apply(0.75), 0.375);
        assert_eq!(first.apply(0.75), 0.0);
    }

    #[test]
    fn channel_faders_apply_after_summing_and_before_independent_sends() {
        let project: Project = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "nodes": [
                { "id": "tone1", "label": "Tone 1", "x": 0, "y": 0,
                    "kind": { "type": "tone", "frequencyHz": 440 } },
                { "id": "tone2", "label": "Tone 2", "x": 0, "y": 200,
                    "kind": { "type": "tone", "frequencyHz": 880 } },
                { "id": "mix", "label": "Canaux de sortie", "x": 300, "y": 0,
                    "kind": { "type": "outputChannels", "channels": 2 } },
                { "id": "virtual", "label": "Virtuel", "x": 600, "y": 0,
                    "kind": { "type": "deviceOutput", "deviceId": "virtual-device" } },
                { "id": "monitor", "label": "Écoute", "x": 600, "y": 200,
                    "kind": { "type": "deviceOutput", "deviceId": "monitor-device" } }
            ],
            "routes": [
                { "id": "publish", "source": "mix", "target": "virtual", "sourceChannel": 1,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "one", "source": "tone1", "target": "mix", "sourceChannel": 0,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "monitor", "source": "mix", "target": "monitor", "sourceChannel": 1,
                    "targetChannel": 1, "gain": 0.5, "muted": false },
                { "id": "two", "source": "tone2", "target": "mix", "sourceChannel": 0,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "reference-in", "source": "tone1", "target": "mix", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 0.5, "muted": false },
                { "id": "reference-out", "source": "mix", "target": "virtual", "sourceChannel": 0,
                    "targetChannel": 1, "gain": 1.0, "muted": false }
            ]
        }))
        .unwrap();
        let mut sources: Vec<_> = project.nodes[..2]
            .iter()
            .map(|node| {
                Source::open(node, project.sample_rate, &BrowserCapture::default()).unwrap()
            })
            .collect();
        for source in &mut sources {
            source.frame[0] = 0.75;
        }
        let mut mixer = Mixer::compile(
            &project,
            &sources,
            [("virtual", "Virtuel", 2), ("monitor", "Écoute", 2)].into_iter(),
        )
        .unwrap();

        let render = |mixer: &mut Mixer| {
            let mut devices = [[0.0; 2]; 2];
            mixer.mix_frame(&sources, |device, channel, sample| {
                devices[device][channel] += sample
            });
            devices
        };
        let mut reported = BTreeMap::new();
        assert_eq!(render(&mut mixer), [[1.0, 0.375], [0.0, 0.5]]);
        mixer.stages[0].meter.report("mix", &mut reported);
        assert_eq!(reported["mix"].channels, [0.375, 1.0]);
        assert_eq!(reported["mix"].clipped_channels, [false, true]);
        assert!(reported["mix"].clipping);

        let mut edited = project.clone();
        let levels = vec![
            ChannelLevel {
                gain: 1.0,
                muted: false,
            },
            ChannelLevel {
                gain: 0.5,
                muted: false,
            },
        ];
        edited.nodes[2].kind = NodeKind::OutputChannels {
            channels: 2,
            levels,
            stereo_pairs: Vec::new(),
            master: None,
        };
        edited.nodes.swap(0, 2);
        let virtual_devices = HashSet::new();
        mixer.set_gains(
            updated_gains(&project, &edited, &virtual_devices).unwrap(),
            1,
        );
        assert_eq!(render(&mut mixer), [[0.75, 0.375], [0.0, 0.375]]);
        mixer.stages[0].meter.report("mix", &mut reported);
        assert_eq!(reported["mix"].channels, [0.375, 0.75]);
        assert_eq!(reported["mix"].clipped_channels, [false, false]);
        assert!(!reported["mix"].clipping);

        let NodeKind::OutputChannels { levels, .. } = &mut edited.nodes[0].kind else {
            unreachable!();
        };
        levels[1].muted = true;
        mixer.set_gains(
            updated_gains(&project, &edited, &virtual_devices).unwrap(),
            2,
        );
        assert_eq!(render(&mut mixer), [[0.375, 0.375], [0.0, 0.1875]]);
        assert_eq!(render(&mut mixer), [[0.0, 0.375], [0.0, 0.0]]);

        let NodeKind::OutputChannels {
            levels,
            stereo_pairs,
            ..
        } = &mut edited.nodes[0].kind
        else {
            unreachable!();
        };
        *stereo_pairs = vec![[0, 1]];
        for level in levels {
            *level = ChannelLevel {
                gain: 0.5,
                muted: false,
            };
        }
        mixer.set_gains(
            updated_gains(&project, &edited, &virtual_devices).unwrap(),
            1,
        );
        assert_eq!(render(&mut mixer), [[0.75, 0.1875], [0.0, 0.375]]);

        edited.nodes[0].kind = NodeKind::OutputChannels {
            channels: 3,
            levels: Vec::new(),
            stereo_pairs: Vec::new(),
            master: None,
        };
        assert!(edited.validate().is_ok());
        assert!(updated_gains(&project, &edited, &virtual_devices).is_err());
    }

    #[test]
    fn master_sums_post_fader_audio_and_feeds_independent_stereo_sends() {
        use crate::graph::{MASTER_LEFT_CHANNEL, MASTER_RIGHT_CHANNEL};
        use std::f32::consts::FRAC_1_SQRT_2;

        let project: Project = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "nodes": [
                { "id": "left", "label": "Gauche", "x": 0, "y": 0,
                    "kind": { "type": "tone", "frequencyHz": 440 } },
                { "id": "right", "label": "Droite", "x": 0, "y": 100,
                    "kind": { "type": "tone", "frequencyHz": 550 } },
                { "id": "mono", "label": "Mono", "x": 0, "y": 200,
                    "kind": { "type": "tone", "frequencyHz": 880 } },
                { "id": "mix", "label": "Canaux de sortie", "x": 300, "y": 0,
                    "kind": { "type": "outputChannels", "channels": 3, "stereoPairs": [[0, 1]],
                        "levels": [{"gain": 2.0, "muted": false}, {"gain": 1.0, "muted": false}, {"gain": 1.0, "muted": false}] } },
                { "id": "output", "label": "Sortie", "x": 600, "y": 0,
                    "kind": { "type": "deviceOutput", "deviceId": "test-device" } }
            ],
            "routes": [
                { "id": "l", "source": "left", "target": "mix", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "r", "source": "right", "target": "mix", "sourceChannel": 0,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "m", "source": "mono", "target": "mix", "sourceChannel": 0,
                    "targetChannel": 2, "gain": 1.0, "muted": false },
                { "id": "master-left", "source": "mix", "target": "output", "sourceChannel": MASTER_LEFT_CHANNEL,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "master-right", "source": "mix", "target": "output", "sourceChannel": MASTER_RIGHT_CHANNEL,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "repeat-left", "source": "mix", "target": "output", "sourceChannel": MASTER_LEFT_CHANNEL,
                    "targetChannel": 2, "gain": 1.0, "muted": false },
                { "id": "direct", "source": "mix", "target": "output", "sourceChannel": 0,
                    "targetChannel": 3, "gain": 1.0, "muted": false }
            ]
        }))
        .unwrap();
        let mut sources: Vec<_> = project.nodes[..3]
            .iter()
            .map(|node| {
                Source::open(node, project.sample_rate, &BrowserCapture::default()).unwrap()
            })
            .collect();
        for (source, sample) in sources.iter_mut().zip([0.8, -0.5, 0.4]) {
            source.frame[0] = sample;
        }
        let devices = HashSet::new();
        let mut mixer =
            Mixer::compile(&project, &sources, [("output", "Sortie", 4)].into_iter()).unwrap();

        let render = |mixer: &mut Mixer| {
            let mut output = [0.0; 4];
            mixer.mix_frame(&sources, |_, channel, sample| output[channel] += sample);
            output
        };
        let mono = 0.4 * FRAC_1_SQRT_2;
        let output = render(&mut mixer);
        assert_eq!(output, [1.0, -0.5 + mono, 1.0, 1.0]);
        let mut reported = BTreeMap::new();
        mixer.stages[0].meter.report("mix", &mut reported);
        assert_eq!(reported["mix"].clipped_channels, [true, false, false]);
        assert_eq!(
            reported["mix"].master.as_ref().unwrap().clipped_channels,
            [true, false]
        );

        let mut edited = project.clone();
        let NodeKind::OutputChannels { master, .. } = &mut edited.nodes[3].kind else {
            unreachable!();
        };
        *master = Some(ChannelLevel {
            gain: 0.5,
            muted: false,
        });
        mixer.set_gains(updated_gains(&project, &edited, &devices).unwrap(), 1);
        let [left, right] = [(1.6 + mono) * 0.5, (-0.5 + mono) * 0.5];
        assert_eq!(render(&mut mixer), [left, right, left, 1.0]);
        mixer.stages[0].meter.report("mix", &mut reported);
        let meter = reported["mix"].master.as_ref().unwrap();
        assert_eq!(meter.channels, [left, right.abs()]);
        assert_eq!(meter.clipped_channels, [false, false]);

        let NodeKind::OutputChannels { master, .. } = &mut edited.nodes[3].kind else {
            unreachable!();
        };
        master.as_mut().unwrap().muted = true;
        mixer.set_gains(updated_gains(&project, &edited, &devices).unwrap(), 2);
        assert_eq!(
            render(&mut mixer),
            [left * 0.5, right * 0.5, left * 0.5, 1.0]
        );
        assert_eq!(render(&mut mixer), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn master_only_clipping_sets_global_meter_without_clipping_tracks() {
        let mut output = OutputStage::new(OutputMix::new(3, &[], &[[0, 1]], None));
        let mut frame = [0.875, -0.125, 0.5];
        let mut meter = Meter::new(frame.len());

        output.finish_frame(&mut frame, &mut meter);

        assert_eq!(frame, [0.875, -0.125, 0.5]);
        assert_eq!(meter.clipped_channels, [false; 3]);
        assert_eq!(
            meter.master.as_ref().unwrap().clipped_channels,
            [true, false]
        );
        assert!(meter.clipping);
    }

    #[test]
    fn chained_buses_deliver_impulses_on_the_same_frame_as_a_direct_send() {
        use crate::graph::{MASTER_LEFT_CHANNEL, MASTER_RIGHT_CHANNEL};

        let project: Project = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "nodes": [
                { "id": "return", "label": "Retour", "x": 0, "y": 0,
                    "kind": { "type": "internalBus", "channels": 2 } },
                { "id": "mix", "label": "Mix", "x": 0, "y": 0,
                    "kind": { "type": "outputChannels", "channels": 2, "stereoPairs": [[0, 1]] } },
                { "id": "group", "label": "Groupe", "x": 0, "y": 0,
                    "kind": { "type": "internalBus", "channels": 2 } },
                { "id": "left", "label": "Gauche", "x": 0, "y": 0,
                    "kind": { "type": "tone", "frequencyHz": 440 } },
                { "id": "right", "label": "Droite", "x": 0, "y": 0,
                    "kind": { "type": "tone", "frequencyHz": 550 } },
                { "id": "extra", "label": "Doublure", "x": 0, "y": 0,
                    "kind": { "type": "tone", "frequencyHz": 660 } },
                { "id": "sink", "label": "Sortie de test", "x": 0, "y": 0,
                    "kind": { "type": "deviceOutput", "deviceId": "test-device" } }
            ],
            "routes": [
                { "id": "return-left", "source": "return", "target": "sink", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "return-right", "source": "return", "target": "sink", "sourceChannel": 1,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "master-left", "source": "mix", "target": "return", "sourceChannel": MASTER_LEFT_CHANNEL,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "master-right", "source": "mix", "target": "return", "sourceChannel": MASTER_RIGHT_CHANNEL,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "attenuate", "source": "group", "target": "mix", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 0.5, "muted": false },
                { "id": "group-right", "source": "group", "target": "mix", "sourceChannel": 1,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "left-in", "source": "left", "target": "group", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "right-in", "source": "right", "target": "group", "sourceChannel": 0,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "extra-in", "source": "extra", "target": "group", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "reference", "source": "left", "target": "sink", "sourceChannel": 0,
                    "targetChannel": 2, "gain": 1.0, "muted": false }
            ]
        }))
        .unwrap();
        project.validate().unwrap();
        let mut sources: Vec<_> = project.nodes[3..6]
            .iter()
            .map(|node| {
                Source::open(node, project.sample_rate, &BrowserCapture::default()).unwrap()
            })
            .collect();
        let mut mixer = Mixer::compile(
            &project,
            &sources,
            [("sink", "Sortie de test", 3)].into_iter(),
        )
        .unwrap();
        let render = |mixer: &mut Mixer, sources: &mut [Source], input: [f32; 3]| {
            for (source, sample) in sources.iter_mut().zip(input) {
                source.frame[0] = sample;
            }
            let mut output = [0.0; 3];
            mixer.mix_frame(sources, |_, channel, sample| output[channel] += sample);
            output
        };
        let impulse = [0.75, -0.25, 0.75];
        assert_eq!(render(&mut mixer, &mut sources, [0.0; 3]), [0.0; 3]);
        assert_eq!(render(&mut mixer, &mut sources, impulse), impulse);
        assert_eq!(render(&mut mixer, &mut sources, [0.0; 3]), [0.0; 3]);
        assert_eq!(mixer.stages[0].meter.clipped_channels, [true, false]);
        assert!(mixer.stages[0].meter.master.is_none());

        let mut edited = project.clone();
        edited.routes[8].muted = true;
        edited.routes.reverse();
        let NodeKind::OutputChannels { levels, .. } = &mut edited.nodes[1].kind else {
            unreachable!();
        };
        *levels = vec![
            ChannelLevel {
                gain: 1.0,
                muted: false,
            },
            ChannelLevel {
                gain: 0.5,
                muted: false,
            },
        ];
        edited.nodes.reverse();
        mixer.set_gains(
            updated_gains(&project, &edited, &HashSet::new()).unwrap(),
            1,
        );
        assert_eq!(
            render(&mut mixer, &mut sources, impulse),
            [0.375, -0.125, 0.75]
        );
    }

    #[test]
    fn an_internal_bus_can_run_and_report_levels_without_audio_devices() {
        let project: Project = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "nodes": [
                { "id": "tone", "label": "Tonalité", "x": 0, "y": 0,
                    "kind": { "type": "tone", "frequencyHz": 1000 } },
                { "id": "bus", "label": "Bus", "x": 0, "y": 0,
                    "kind": { "type": "internalBus", "channels": 2 } }
            ],
            "routes": [
                { "id": "right", "source": "tone", "target": "bus", "sourceChannel": 0,
                    "targetChannel": 1, "gain": 1.0, "muted": false }
            ]
        }))
        .unwrap();
        project.validate().unwrap();
        let (mut plan, virtual_devices) =
            RenderPlan::open(&project, &BrowserCapture::default()).unwrap();
        assert!(plan.destinations.is_empty());
        assert!(virtual_devices.is_empty());
        plan.render(240);
        let status = Mutex::new(EngineStatus::default());
        plan.report(&status);
        let status = status.into_inner().unwrap();
        assert_eq!(status.meters["bus"].channels, [0.0, TONE_LEVEL]);
        assert_eq!(status.dropped_frames, 0);

        let runtime = Runtime::start(project, BrowserCapture::default()).unwrap();
        let status = runtime.status();
        assert!(status.running);
        assert_eq!(status.realtime, cfg!(target_os = "macos"));
        assert_eq!(
            serde_json::to_value(&status).unwrap()["realtime"],
            cfg!(target_os = "macos")
        );
        runtime.stop();
    }

    #[test]
    fn stereo_changes_cannot_create_live_feedback_through_the_master() {
        use crate::graph::{MASTER_LEFT_CHANNEL, MASTER_RIGHT_CHANNEL};

        let project: Project = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "nodes": [
                { "id": "input", "label": "Loopback", "x": 0, "y": 0,
                    "kind": { "type": "deviceInput", "deviceId": "BlackHole2ch_UID" } },
                { "id": "mix", "label": "Canaux de sortie", "x": 300, "y": 0,
                    "kind": { "type": "outputChannels", "channels": 2, "stereoPairs": [[0, 1]] } },
                { "id": "output", "label": "Bus", "x": 600, "y": 0,
                    "kind": { "type": "deviceOutput", "deviceId": "fr.patchbay.audio.bus.BlackHole2ch_UID" } }
            ],
            "routes": [
                { "id": "return", "source": "input", "target": "mix", "sourceChannel": 0,
                    "targetChannel": 1, "gain": 1.0, "muted": false },
                { "id": "send", "source": "mix", "target": "output", "sourceChannel": MASTER_LEFT_CHANNEL,
                    "targetChannel": 0, "gain": 1.0, "muted": false }
            ]
        }))
        .unwrap();
        let virtual_devices = HashSet::from([String::from("BlackHole2ch_UID")]);
        assert!(updated_gains(&project, &project, &virtual_devices).is_ok());
        let mut edited = project.clone();
        let NodeKind::OutputChannels { stereo_pairs, .. } = &mut edited.nodes[1].kind else {
            unreachable!();
        };
        stereo_pairs.clear();
        assert!(updated_gains(&project, &edited, &virtual_devices).is_err());

        let mut right_send = project.clone();
        right_send.routes[1].source_channel = MASTER_RIGHT_CHANNEL;
        let virtual_ids = virtual_devices.iter().map(String::as_str).collect();
        assert!(right_send.reject_virtual_feedback(&virtual_ids).is_err());
        right_send.routes[1].target_channel = 1;
        assert!(right_send.reject_virtual_feedback(&virtual_ids).is_ok());
    }

    #[test]
    fn interpolation_preserves_channel_separation() {
        let mut mixed = [0.0; 2];
        interpolate(&[1.0, -0.5], &[0.0, 0.5], 0.25, &mut mixed);
        assert_eq!(mixed, [0.75, -0.25]);
    }

    #[test]
    fn live_edits_follow_route_ids_without_changing_the_running_topology() {
        let project: Project = serde_json::from_value(serde_json::json!({
            "sampleRate": 48000,
            "nodes": [
                { "id": "source", "label": "App", "x": 0, "y": 0,
                    "kind": { "type": "application", "bundleId": "audio.example.app" } },
                { "id": "sink", "label": "Bus", "x": 400, "y": 0,
                    "kind": { "type": "deviceOutput", "deviceId": "BlackHole_UID" } }
            ],
            "routes": [
                { "id": "left", "source": "source", "target": "sink", "sourceChannel": 0,
                    "targetChannel": 0, "gain": 1.0, "muted": false },
                { "id": "right", "source": "source", "target": "sink", "sourceChannel": 1,
                    "targetChannel": 1, "gain": 1.0, "muted": false }
            ]
        }))
        .unwrap();
        let mut edited = project.clone();
        edited.routes.swap(0, 1);
        edited.routes[0].muted = true;
        edited.routes[1].gain = 0.5;
        edited.nodes[0].x = 200.0;
        let virtual_devices = HashSet::new();
        assert_eq!(
            updated_gains(&project, &edited, &virtual_devices)
                .unwrap()
                .routes,
            [0.5, 0.0]
        );
        edited.routes[0].target_channel = 0;
        assert!(updated_gains(&project, &edited, &virtual_devices).is_err());
    }
}

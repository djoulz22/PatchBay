use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::f32::consts::FRAC_1_SQRT_2;

use serde::{Deserialize, Serialize};

pub const MAX_CHANNELS: usize = 64;
// These source-only wire indices are independent of the block's channel count.
pub const MASTER_LEFT_CHANNEL: usize = 64;
pub const MASTER_RIGHT_CHANNEL: usize = 65;
const MAX_NODES: usize = 64;
const MAX_ROUTES: usize = 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub sample_rate: u32,
    pub nodes: Vec<GraphNode>,
    pub routes: Vec<Route>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            nodes: Vec::new(),
            routes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub kind: NodeKind,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NodeKind {
    Tone {
        frequency_hz: f64,
    },
    DeviceInput {
        device_id: String,
    },
    SystemAudio,
    Application {
        bundle_id: String,
        #[serde(default = "default_mute_original", skip_serializing_if = "is_true")]
        mute_original: bool,
    },
    BrowserTab {
        tab_id: String,
    },
    InternalBus {
        channels: usize,
    },
    OutputChannels {
        channels: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        levels: Vec<ChannelLevel>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        stereo_pairs: Vec<[usize; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        master: Option<ChannelLevel>,
    },
    DeviceOutput {
        device_id: String,
    },
}

fn default_mute_original() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

pub(crate) fn valid_tab_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

impl NodeKind {
    pub(crate) fn is_stage(&self) -> bool {
        matches!(self, Self::InternalBus { .. } | Self::OutputChannels { .. })
    }

    fn channel_limit(&self) -> usize {
        match self {
            Self::Tone { .. } => 1,
            Self::SystemAudio | Self::Application { .. } | Self::BrowserTab { .. } => 2,
            Self::InternalBus { channels } | Self::OutputChannels { channels, .. } => *channels,
            Self::DeviceInput { .. } | Self::DeviceOutput { .. } => MAX_CHANNELS,
        }
    }

    fn has_source_channel(&self, channel: usize) -> bool {
        channel < self.channel_limit()
            || (matches!(self, Self::OutputChannels { .. }) && master_side(channel).is_some())
    }

    fn can_connect_to(&self, target: &Self) -> bool {
        match self {
            Self::DeviceOutput { .. } => false,
            Self::OutputChannels { .. } => {
                matches!(target, Self::InternalBus { .. } | Self::DeviceOutput { .. })
            }
            _ => target.is_stage() || matches!(target, Self::DeviceOutput { .. }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChannelLevel {
    pub gain: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: String,
    pub source: String,
    pub target: String,
    pub source_channel: usize,
    pub target_channel: usize,
    pub gain: f32,
    pub muted: bool,
}

impl Route {
    pub(crate) fn ports(&self) -> (&str, &str, usize, usize) {
        (
            &self.source,
            &self.target,
            self.source_channel,
            self.target_channel,
        )
    }
}

impl Project {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.sample_rate, 44_100 | 48_000 | 96_000) {
            return Err("Fréquence du projet invalide. Choisis 44,1, 48 ou 96 kHz.".into());
        }
        if self.nodes.len() > MAX_NODES || self.routes.len() > MAX_ROUTES {
            return Err(format!(
                "Le projet dépasse {MAX_NODES} blocs ou {MAX_ROUTES} liaisons."
            ));
        }

        let mut nodes = HashMap::new();
        let mut browser_tabs = HashSet::new();
        for node in &self.nodes {
            if !valid_text(&node.id, 128) || !valid_text(&node.label, 256) {
                return Err("Un bloc a un identifiant ou un nom invalide.".into());
            }
            if !node.x.is_finite() || !node.y.is_finite() {
                return Err(format!("Position invalide pour « {} ».", node.label));
            }
            match &node.kind {
                NodeKind::Tone { frequency_hz } if !(20.0..=20_000.0).contains(frequency_hz) => {
                    return Err(format!(
                        "Fréquence de test invalide pour « {} ».",
                        node.label
                    ));
                }
                NodeKind::DeviceInput { device_id } | NodeKind::DeviceOutput { device_id }
                    if !valid_text(device_id, 1024) =>
                {
                    return Err(format!(
                        "Identifiant de périphérique invalide pour « {} ».",
                        node.label
                    ));
                }
                NodeKind::Application { bundle_id, .. } if !valid_text(bundle_id, 1024) => {
                    return Err(format!(
                        "Identifiant d'application invalide pour « {} ».",
                        node.label
                    ));
                }
                NodeKind::BrowserTab { tab_id } => {
                    if !valid_tab_id(tab_id) || !browser_tabs.insert(tab_id.as_str()) {
                        return Err(format!(
                            "Identifiant d'onglet invalide ou en double pour « {} ».",
                            node.label
                        ));
                    }
                }
                NodeKind::InternalBus { channels } | NodeKind::OutputChannels { channels, .. }
                    if !(1..=MAX_CHANNELS).contains(channels) =>
                {
                    return Err(format!(
                        "« {} » doit comporter entre 1 et {MAX_CHANNELS} canaux.",
                        node.label
                    ));
                }
                NodeKind::OutputChannels {
                    channels,
                    levels,
                    stereo_pairs,
                    master,
                } => {
                    if !levels.is_empty() && levels.len() != *channels {
                        return Err(format!(
                            "« {} » doit comporter un réglage de niveau par canal.",
                            node.label
                        ));
                    }
                    if levels
                        .iter()
                        .any(|level| !(0.0..=4.0).contains(&level.gain))
                    {
                        return Err(format!(
                            "Gain de canal hors limites dans « {} ».",
                            node.label
                        ));
                    }
                    let mut paired = HashSet::new();
                    if stereo_pairs
                        .iter()
                        .flatten()
                        .any(|&channel| channel >= *channels || !paired.insert(channel))
                    {
                        return Err(format!(
                            "Paires stéréo invalides dans « {} ». Utilise des canaux distincts et disponibles.",
                            node.label
                        ));
                    }
                    if master
                        .as_ref()
                        .is_some_and(|level| !(0.0..=4.0).contains(&level.gain))
                    {
                        return Err(format!(
                            "Gain du Master hors limites dans « {} ».",
                            node.label
                        ));
                    }
                }
                _ => {}
            }
            if nodes.insert(node.id.as_str(), node).is_some() {
                return Err(format!("Identifiant de bloc dupliqué « {} ».", node.id));
            }
        }

        let mut route_ids = HashSet::new();
        let mut connections = HashSet::new();
        for route in &self.routes {
            if !valid_text(&route.id, 128) || !route_ids.insert(route.id.as_str()) {
                return Err("Un identifiant de liaison est invalide ou dupliqué.".into());
            }
            let source = nodes
                .get(route.source.as_str())
                .ok_or_else(|| format!("Source introuvable pour la liaison « {} ».", route.id))?;
            let target = nodes
                .get(route.target.as_str())
                .ok_or_else(|| format!("Sortie introuvable pour la liaison « {} ».", route.id))?;
            if !source.kind.can_connect_to(&target.kind) {
                return Err(format!(
                    "La direction de la liaison entre « {} » et « {} » est invalide.",
                    source.label, target.label
                ));
            }
            if !source.kind.has_source_channel(route.source_channel)
                || route.target_channel >= target.kind.channel_limit()
            {
                return Err(format!(
                    "Canal invalide pour la liaison entre « {} » et « {} ».",
                    source.label, target.label
                ));
            }
            if !(0.0..=4.0).contains(&route.gain) {
                return Err(format!(
                    "Gain hors limites pour la liaison entre « {} » et « {} ».",
                    source.label, target.label
                ));
            }
            if !connections.insert(route.ports()) {
                return Err(format!(
                    "La liaison entre « {} » et « {} » existe déjà.",
                    source.label, target.label
                ));
            }
        }
        self.ordered_stages()?;
        Ok(())
    }

    pub(crate) fn connected_nodes(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes.iter().filter(|node| {
            self.routes
                .iter()
                .any(|route| route.source == node.id || route.target == node.id)
        })
    }

    pub(crate) fn ordered_stages(&self) -> Result<Vec<&GraphNode>, String> {
        let stages: Vec<_> = self
            .connected_nodes()
            .filter(|node| node.kind.is_stage())
            .collect();
        let indices: HashMap<_, _> = stages
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        let mut incoming = vec![0; stages.len()];
        let mut downstream = vec![Vec::new(); stages.len()];
        for route in &self.routes {
            if let (Some(&source), Some(&target)) = (
                indices.get(route.source.as_str()),
                indices.get(route.target.as_str()),
            ) {
                downstream[source].push(target);
                incoming[target] += 1;
            }
        }
        let mut ready: BinaryHeap<_> = incoming
            .iter()
            .enumerate()
            .filter_map(|(index, &count)| (count == 0).then_some(Reverse(index)))
            .collect();
        let mut ordered = Vec::with_capacity(stages.len());
        while let Some(Reverse(source)) = ready.pop() {
            ordered.push(stages[source]);
            for &target in &downstream[source] {
                incoming[target] -= 1;
                if incoming[target] == 0 {
                    ready.push(Reverse(target));
                }
            }
        }
        if ordered.len() != stages.len() {
            return Err("Le routage contient une boucle entre des blocs de mixage.".into());
        }
        Ok(ordered)
    }

    pub(crate) fn reject_virtual_feedback(
        &self,
        virtual_ids: &HashSet<&str>,
    ) -> Result<(), String> {
        let virtual_ids: HashSet<_> = virtual_ids
            .iter()
            .map(|id| canonical_device_id(id))
            .collect();
        let nodes: HashMap<_, _> = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), &node.kind))
            .collect();
        let mut links: HashMap<FeedbackPort<'_>, Vec<FeedbackPort<'_>>> = HashMap::new();
        let mut master_stages = HashSet::new();
        for route in &self.routes {
            let source = feedback_port(
                &route.source,
                nodes[route.source.as_str()],
                route.source_channel,
                &virtual_ids,
            );
            let target = feedback_port(
                &route.target,
                nodes[route.target.as_str()],
                route.target_channel,
                &virtual_ids,
            );
            if let (Some(source), Some(target)) = (source, target) {
                if let FeedbackPort::Stage(stage, channel) = source
                    && master_side(channel).is_some()
                {
                    master_stages.insert(stage);
                }
                links.entry(source).or_default().push(target);
            }
        }
        for stage in master_stages {
            let NodeKind::OutputChannels {
                channels,
                stereo_pairs,
                ..
            } = nodes[stage]
            else {
                continue;
            };
            for (channel, weights) in master_pan(*channels, stereo_pairs).into_iter().enumerate() {
                let targets = links
                    .entry(FeedbackPort::Stage(stage, channel))
                    .or_default();
                for (weight, master_channel) in weights
                    .into_iter()
                    .zip([MASTER_LEFT_CHANNEL, MASTER_RIGHT_CHANNEL])
                {
                    if weight > 0.0 {
                        targets.push(FeedbackPort::Stage(stage, master_channel));
                    }
                }
            }
        }

        let mut incoming = HashMap::<_, usize>::new();
        for (source, targets) in &links {
            incoming.entry(*source).or_default();
            for target in targets {
                *incoming.entry(*target).or_default() += 1;
            }
        }
        let mut ready: Vec<_> = incoming
            .iter()
            .filter_map(|(port, &count)| (count == 0).then_some(*port))
            .collect();
        let mut visited = 0;
        while let Some(source) = ready.pop() {
            visited += 1;
            if let Some(targets) = links.get(&source) {
                for target in targets {
                    let count = incoming.get_mut(target).expect("connected target");
                    *count -= 1;
                    if *count == 0 {
                        ready.push(*target);
                    }
                }
            }
        }
        if visited != incoming.len() {
            return Err(
                "Le routage contient une boucle entre canaux de périphériques virtuels.".into(),
            );
        }
        Ok(())
    }
}

pub(crate) fn master_side(channel: usize) -> Option<usize> {
    match channel {
        MASTER_LEFT_CHANNEL => Some(0),
        MASTER_RIGHT_CHANNEL => Some(1),
        _ => None,
    }
}

pub(crate) fn master_pan(channels: usize, stereo_pairs: &[[usize; 2]]) -> Vec<[f32; 2]> {
    let mut pan = vec![[FRAC_1_SQRT_2; 2]; channels];
    for &[left, right] in stereo_pairs {
        pan[left] = [1.0, 0.0];
        pan[right] = [0.0, 1.0];
    }
    pan
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum FeedbackPort<'a> {
    Device(&'a str, usize),
    Stage(&'a str, usize),
}

fn feedback_port<'a>(
    node_id: &'a str,
    kind: &'a NodeKind,
    channel: usize,
    virtual_ids: &HashSet<&str>,
) -> Option<FeedbackPort<'a>> {
    match kind {
        NodeKind::DeviceInput { device_id } | NodeKind::DeviceOutput { device_id } => {
            let id = canonical_device_id(device_id);
            virtual_ids
                .contains(id)
                .then_some(FeedbackPort::Device(id, channel))
        }
        kind if kind.is_stage() => Some(FeedbackPort::Stage(node_id, channel)),
        _ => None,
    }
}

pub(crate) fn canonical_device_id(id: &str) -> &str {
    crate::virtual_bus::backing_id(id).unwrap_or(id)
}

fn valid_text(value: &str, max_len: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_len && !value.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_node(id: &str, device_id: &str, is_input: bool) -> GraphNode {
        GraphNode {
            id: id.into(),
            label: id.into(),
            x: 0.,
            y: 0.,
            kind: if is_input {
                NodeKind::DeviceInput {
                    device_id: device_id.into(),
                }
            } else {
                NodeKind::DeviceOutput {
                    device_id: device_id.into(),
                }
            },
        }
    }

    fn route(source: &str, target: &str, source_channel: usize, target_channel: usize) -> Route {
        Route {
            id: format!("{source}-{target}-{source_channel}-{target_channel}"),
            source: source.into(),
            target: target.into(),
            source_channel,
            target_channel,
            gain: 1.0,
            muted: false,
        }
    }

    fn output_channels(id: &str, channels: usize) -> GraphNode {
        GraphNode {
            id: id.into(),
            label: id.into(),
            x: 0.0,
            y: 0.0,
            kind: NodeKind::OutputChannels {
                channels,
                levels: Vec::new(),
                stereo_pairs: Vec::new(),
                master: None,
            },
        }
    }

    fn internal_bus(id: &str, channels: usize) -> GraphNode {
        GraphNode {
            id: id.into(),
            label: id.into(),
            x: 0.0,
            y: 0.0,
            kind: NodeKind::InternalBus { channels },
        }
    }

    #[test]
    fn internal_buses_round_trip_with_numbered_duplex_ports() {
        let bus = serde_json::json!({ "type": "internalBus", "channels": 64 });
        let mut project = Project {
            nodes: vec![
                device_node("input", "device-in", true),
                internal_bus("bus", 64),
                output_channels("mix", 2),
                device_node("output", "device-out", false),
            ],
            routes: vec![
                route("input", "bus", 0, 63),
                route("bus", "mix", 63, 1),
                route("mix", "output", MASTER_RIGHT_CHANNEL, 0),
            ],
            ..Project::default()
        };
        project.nodes[1].kind = serde_json::from_value(bus.clone()).unwrap();
        assert_eq!(serde_json::to_value(&project.nodes[1].kind).unwrap(), bus);
        assert!(project.validate().is_ok());

        project.routes[1].source_channel = MASTER_LEFT_CHANNEL;
        assert!(project.validate().is_err());
        project.routes[1].source_channel = 63;
        project.routes[0].target_channel = MASTER_RIGHT_CHANNEL;
        assert!(project.validate().is_err());
        project.routes[0].target_channel = 63;
        for channels in [0, MAX_CHANNELS + 1] {
            project.nodes[1].kind = NodeKind::InternalBus { channels };
            assert!(project.validate().is_err());
        }
    }

    #[test]
    fn orders_connected_stages_and_rejects_interstage_feedback() {
        let mut project = Project {
            nodes: vec![
                internal_bus("second", 2),
                internal_bus("unused", 2),
                output_channels("mix", 2),
                internal_bus("first", 2),
            ],
            routes: vec![
                route("first", "mix", 0, 0),
                route("mix", "second", MASTER_LEFT_CHANNEL, 1),
                route("first", "second", 1, 0),
            ],
            ..Project::default()
        };
        assert!(project.validate().is_ok());
        let order: Vec<_> = project
            .ordered_stages()
            .unwrap()
            .into_iter()
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(order, ["first", "mix", "second"]);

        project.routes.push(route("second", "first", 1, 1));
        assert!(project.validate().is_err());
        project.routes.pop();
        project.routes.push(route("second", "second", 0, 1));
        assert!(project.validate().is_err());
    }

    #[test]
    fn internal_buses_preserve_channel_granularity_of_virtual_feedback() {
        let mut project = Project {
            nodes: vec![
                device_node("driver", "BlackHole2ch_UID", true),
                internal_bus("first", 2),
                internal_bus("second", 2),
                device_node("alias", "fr.patchbay.audio.bus.BlackHole2ch_UID", false),
            ],
            routes: vec![
                route("driver", "first", 0, 1),
                route("first", "second", 1, 0),
                route("second", "alias", 0, 0),
            ],
            ..Project::default()
        };
        let virtuals = HashSet::from(["BlackHole2ch_UID"]);
        assert!(project.validate().is_ok());
        assert!(project.reject_virtual_feedback(&virtuals).is_err());
        project.routes[2].target_channel = 1;
        assert!(project.reject_virtual_feedback(&virtuals).is_ok());
    }

    #[test]
    fn application_sources_default_to_muting_and_keep_an_explicit_opt_out() {
        let legacy = serde_json::json!({
            "type": "application", "bundleId": "com.spotify.client"
        });
        let mut opted_out = legacy.clone();
        opted_out["muteOriginal"] = serde_json::json!(false);
        let mut explicit_mute = legacy.clone();
        explicit_mute["muteOriginal"] = serde_json::json!(true);

        let legacy_source: NodeKind = serde_json::from_value(legacy.clone()).unwrap();
        let muted_source: NodeKind = serde_json::from_value(explicit_mute).unwrap();
        let direct_source: NodeKind = serde_json::from_value(opted_out.clone()).unwrap();

        assert_eq!(legacy_source, muted_source);
        assert_ne!(legacy_source, direct_source);
        assert_eq!(serde_json::to_value(legacy_source).unwrap(), legacy);
        assert_eq!(serde_json::to_value(direct_source).unwrap(), opted_out);
    }

    #[test]
    fn rejects_duplicate_mappings_and_reversed_connections() {
        let mut project = Project {
            nodes: vec![
                device_node("in", "device", true),
                device_node("out", "device", false),
            ],
            routes: vec![route("in", "out", 0, 1)],
            ..Project::default()
        };
        assert!(project.validate().is_ok());
        project.routes.push(Route {
            id: "another".into(),
            ..project.routes[0].clone()
        });
        assert!(project.validate().is_err());
        project.routes[1] = route("out", "in", 0, 1);
        assert!(project.validate().is_err());
    }

    #[test]
    fn detects_feedback_across_virtual_devices_at_channel_granularity() {
        let mut project = Project {
            nodes: vec![
                device_node("a-in", "a", true),
                device_node("a-out", "a", false),
                device_node("b-in", "b", true),
                device_node("b-out", "b", false),
            ],
            routes: vec![route("a-in", "b-out", 0, 1), route("b-in", "a-out", 1, 0)],
            ..Project::default()
        };
        let virtuals = HashSet::from(["a", "b"]);
        assert!(project.validate().is_ok());
        assert!(project.reject_virtual_feedback(&virtuals).is_err());
        assert!(
            project
                .reject_virtual_feedback(&HashSet::from(["a"]))
                .is_ok()
        );
        project.routes[1].target_channel = 1;
        assert!(project.reject_virtual_feedback(&virtuals).is_ok());
        project.routes.push(route("a-in", "a-out", 4, 4));
        assert!(project.reject_virtual_feedback(&virtuals).is_err());
    }

    #[test]
    fn treats_virtual_buses_and_their_backing_devices_as_the_same_ports() {
        let project = Project {
            nodes: vec![
                device_node("driver", "BlackHole2ch_UID", true),
                device_node("alias", "fr.patchbay.audio.bus.BlackHole2ch_UID", false),
            ],
            routes: vec![route("driver", "alias", 0, 0)],
            ..Project::default()
        };
        assert!(
            project
                .reject_virtual_feedback(&HashSet::from(["BlackHole2ch_UID"]))
                .is_err()
        );
    }

    #[test]
    fn validates_both_sides_of_the_output_channels_stage() {
        let mut project = Project {
            nodes: vec![
                device_node("input", "device-a", true),
                output_channels("stage", 2),
                output_channels("second-stage", 1),
                device_node("output", "device-b", false),
            ],
            routes: vec![
                route("input", "stage", 2, 1),
                route("stage", "output", 1, 3),
            ],
            ..Project::default()
        };
        assert!(project.validate().is_ok());
        project.routes[0].target_channel = 2;
        assert!(project.validate().is_err());
        project.routes[0].target_channel = 1;
        project.routes[1].source_channel = 2;
        assert!(project.validate().is_err());
        project.routes[1] = route("stage", "second-stage", 1, 0);
        assert!(project.validate().is_err());
        project.routes.pop();
        project.nodes[1].kind = NodeKind::OutputChannels {
            channels: 65,
            levels: Vec::new(),
            stereo_pairs: Vec::new(),
            master: None,
        };
        assert!(project.validate().is_err());
    }

    #[test]
    fn accepts_legacy_levels_and_validates_saved_faders() {
        let legacy = serde_json::json!({ "type": "outputChannels", "channels": 2 });
        let mut project = Project {
            nodes: vec![output_channels("stage", 2)],
            ..Project::default()
        };
        project.nodes[0].kind = serde_json::from_value(legacy.clone()).unwrap();
        assert!(project.validate().is_ok());
        assert_eq!(
            serde_json::to_value(&project.nodes[0].kind).unwrap(),
            legacy
        );

        let with_levels = |levels: Vec<ChannelLevel>| NodeKind::OutputChannels {
            channels: 2,
            levels,
            stereo_pairs: Vec::new(),
            master: None,
        };
        let fader = ChannelLevel {
            gain: 0.5,
            muted: true,
        };
        project.nodes[0].kind = with_levels(vec![fader.clone()]);
        assert!(project.validate().is_err());
        project.nodes[0].kind = with_levels(vec![fader.clone(), fader.clone()]);
        assert!(project.validate().is_ok());
        let saved = serde_json::to_value(&project.nodes[0].kind).unwrap();
        assert_eq!(
            saved["levels"][0],
            serde_json::json!({ "gain": 0.5, "muted": true })
        );
        project.nodes[0].kind = with_levels(vec![
            fader,
            ChannelLevel {
                gain: f32::NAN,
                muted: false,
            },
        ]);
        assert!(project.validate().is_err());
    }

    #[test]
    fn stereo_pairs_preserve_channel_order_and_require_unique_ports() {
        let valid = serde_json::json!({
            "type": "outputChannels",
            "channels": 4,
            "stereoPairs": [[2, 0], [1, 3]],
        });
        let mut project = Project {
            nodes: vec![output_channels("stage", 4)],
            ..Project::default()
        };
        project.nodes[0].kind = serde_json::from_value(valid.clone()).unwrap();
        assert!(project.validate().is_ok());
        assert_eq!(serde_json::to_value(&project.nodes[0].kind).unwrap(), valid);

        for stereo_pairs in [vec![[0, 0]], vec![[0, 4]], vec![[0, 1], [1, 2]]] {
            project.nodes[0].kind = NodeKind::OutputChannels {
                channels: 4,
                levels: Vec::new(),
                stereo_pairs,
                master: None,
            };
            assert!(project.validate().is_err());
        }
    }

    #[test]
    fn master_has_two_stable_source_ports_and_its_own_validated_fader() {
        let mut project = Project {
            nodes: vec![
                device_node("input", "device-a", true),
                output_channels("stage", 1),
                device_node("output", "device-b", false),
            ],
            routes: vec![
                route("input", "stage", 0, 0),
                route("stage", "output", MASTER_LEFT_CHANNEL, 0),
                route("stage", "output", MASTER_RIGHT_CHANNEL, 1),
            ],
            ..Project::default()
        };
        let saved = serde_json::json!({
            "type": "outputChannels", "channels": 1, "master": { "gain": 0.5, "muted": false }
        });
        project.nodes[1].kind = serde_json::from_value(saved.clone()).unwrap();
        assert!(project.validate().is_ok());
        assert_eq!(serde_json::to_value(&project.nodes[1].kind).unwrap(), saved);

        project.routes[0].target_channel = MASTER_LEFT_CHANNEL;
        assert!(project.validate().is_err());
        project.routes[0].target_channel = 0;
        project.routes[0].source_channel = MASTER_LEFT_CHANNEL;
        assert!(project.validate().is_err());
        project.routes[0].source_channel = 0;

        project.routes[2].source_channel = MASTER_RIGHT_CHANNEL + 1;
        assert!(project.validate().is_err());
        project.routes[2].source_channel = MASTER_RIGHT_CHANNEL;
        let NodeKind::OutputChannels { master, .. } = &mut project.nodes[1].kind else {
            unreachable!();
        };
        master.as_mut().unwrap().gain = 4.1;
        assert!(project.validate().is_err());
    }

    #[test]
    fn detects_virtual_feedback_through_an_output_channels_stage() {
        let mut project = Project {
            nodes: vec![
                device_node("driver", "BlackHole2ch_UID", true),
                output_channels("stage", 2),
                device_node("alias", "fr.patchbay.audio.bus.BlackHole2ch_UID", false),
            ],
            routes: vec![
                route("driver", "stage", 0, 1),
                route("stage", "alias", 1, 0),
            ],
            ..Project::default()
        };
        let virtuals = HashSet::from(["BlackHole2ch_UID"]);
        assert!(project.validate().is_ok());
        assert!(project.reject_virtual_feedback(&virtuals).is_err());
        project.routes[1].source_channel = 0;
        assert!(project.reject_virtual_feedback(&virtuals).is_ok());
        project.routes[1].source_channel = 1;
        project.routes[1].target_channel = 1;
        assert!(project.reject_virtual_feedback(&virtuals).is_ok());
    }
}

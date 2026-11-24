use crate::graph::{NodeKind, Project};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, io, path::PathBuf, sync::Mutex};

const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    version: u32,
    active_id: String,
    graphs: Vec<SavedGraph>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedGraph {
    id: String,
    name: String,
    project: Project,
}

impl Session {
    fn verify(&self) -> Result<(), String> {
        if self.version != FORMAT_VERSION {
            return Err(format!("Format de session inconnu ({})", self.version));
        }
        let mut graph_ids = HashSet::new();
        for graph in &self.graphs {
            if graph.id.trim().is_empty()
                || graph.name.trim().is_empty()
                || !graph_ids.insert(graph.id.as_str())
            {
                return Err("Le nom ou l’identifiant d’un graphe est invalide.".into());
            }
            graph
                .project
                .validate()
                .map_err(|error| format!("{} : {error}", graph.name))?;
        }
        if !graph_ids.contains(self.active_id.as_str()) {
            return Err("Le graphe actif manque dans la session.".into());
        }
        Ok(())
    }
}

pub struct SessionStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl SessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    pub fn load(&self) -> Result<Option<Session>, String> {
        let _guard = self.lock.lock().map_err(storage_error)?;
        self.read()
    }

    fn read(&self) -> Result<Option<Session>, String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        let session: Session = serde_json::from_slice(&bytes).map_err(storage_error)?;
        session.verify()?;
        Ok(Some(session))
    }

    pub fn while_device_unreferenced<T>(
        &self,
        id: &str,
        change: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self.lock.lock().map_err(storage_error)?;
        if let Some(session) = self.read()?
            && let Some(graph) = session.graphs.iter().find(|graph| {
                graph.project.nodes.iter().any(|node| {
                    matches!(&node.kind, NodeKind::DeviceInput { device_id } | NodeKind::DeviceOutput { device_id } if device_id == id)
                })
            })
        {
            return Err(format!(
                "Retire d'abord cette interface de la configuration « {} ».",
                graph.name
            ));
        }
        change()
    }

    pub fn save(&self, session: &Session) -> Result<(), String> {
        session.verify()?;
        let _guard = self.lock.lock().map_err(storage_error)?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "Le dossier de session est invalide.".to_string())?;
        fs::create_dir_all(parent).map_err(storage_error)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(storage_error)?;
        serde_json::to_writer_pretty(temp.as_file_mut(), session).map_err(storage_error)?;
        temp.as_file().sync_all().map_err(storage_error)?;
        temp.persist(&self.path).map_err(storage_error)?;
        Ok(())
    }
}

fn storage_error(error: impl std::fmt::Display) -> String {
    format!("Session locale inaccessible ({error}).")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_a_session_preserves_the_last_valid_document() {
        let folder = tempfile::tempdir().unwrap();
        let store = SessionStore::new(folder.path().join("session.json"));
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "version": 1,
            "activeId": "a",
            "graphs": [{
                "id": "a",
                "name": "Studio",
                "project": {"sampleRate": 48000, "nodes": [], "routes": []}
            }]
        }))
        .unwrap();
        assert!(store.load().unwrap().is_none());
        store.save(&session).unwrap();
        session.graphs[0].name = "Studio B".into();
        store.save(&session).unwrap();
        session.active_id = "missing".into();
        assert!(store.save(&session).is_err());
        assert_eq!(store.load().unwrap().unwrap().graphs[0].name, "Studio B");
    }

    #[test]
    fn an_interface_used_by_an_inactive_configuration_cannot_be_deleted() {
        let folder = tempfile::tempdir().unwrap();
        let store = SessionStore::new(folder.path().join("session.json"));
        let interface = "fr.patchbay.audio.native.267dd7e9-0814-49af-9159-7bf801204578";
        let session: Session = serde_json::from_value(serde_json::json!({
            "version": 1,
            "activeId": "main",
            "graphs": [
                {"id": "main", "name": "Studio", "project": {"sampleRate": 48000, "nodes": [], "routes": []}},
                {"id": "old", "name": "Archive", "project": {"sampleRate": 48000, "nodes": [
                    {"id": "source", "label": "Interface", "x": 0, "y": 0,
                     "kind": {"type": "deviceInput", "deviceId": interface}}
                ], "routes": []}}
            ]
        }))
        .unwrap();
        store.save(&session).unwrap();

        let error = store
            .while_device_unreferenced(interface, || -> Result<(), String> {
                panic!("Referenced interface reached the delete operation")
            })
            .unwrap_err();
        assert!(error.contains("Archive"));
        assert_eq!(store.while_device_unreferenced("unused", || Ok(42)), Ok(42));
    }
}

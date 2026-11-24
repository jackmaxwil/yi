//! The app's own settings file through Rune's `Prefs`, apart from the grid's places file: two
//! stores on one file rewrite each other's keys away.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use rune::prefs::{Prefs, PrefsError};
use yd_core::ids::SettingKey;
use yd_core::msg::{Msg, step};
use yd_core::settings::schema::{Setting, Store};
use yd_core::settings::store::SettingsMsg;
use yd_core::settings::store::value::{Json, Refused, read, to_json};
use yd_ui::disk::Disk as Worker;
use yd_ui::win::Hub;

const APP: &str = "yi-desktop-settings";

/// What the UI thread hands the worker: the newest value of each key not yet written.
#[derive(Default)]
struct Pending {
    values: BTreeMap<String, Option<Json>>,
    /// A write job is queued and will take `values`, so another need not be.
    queued: bool,
    /// What the last write met, said on the next change.
    failed: Option<PrefsError>,
}

/// The store in a hub's `panes` under no pane. Until the app installs its file with
/// [`restore`] it keeps nothing past the run.
pub struct Disk {
    /// Invariant: written by the disk worker alone once the store is installed.
    prefs: Arc<Mutex<Prefs>>,
    pending: Arc<Mutex<Pending>>,
    path: Option<PathBuf>,
    /// What the file held that its setting refuses, by key, the default kept.
    pub refused: Vec<Refused>,
}

impl Default for Disk {
    fn default() -> Self {
        Self::with(Prefs::memory())
    }
}

impl Disk {
    fn with(prefs: Prefs) -> Self {
        Self {
            path: prefs.path().map(PathBuf::from),
            prefs: Arc::new(Mutex::new(prefs)),
            pending: Arc::default(),
            refused: Vec::new(),
        }
    }

    /// The app's file in its config folder.
    pub fn app() -> Self {
        Self::with(Prefs::open(APP))
    }

    pub fn at(file: impl Into<PathBuf>) -> Self {
        Self::with(Prefs::at(file))
    }

    /// The folder the file is in, where the app keeps its other files beside it.
    pub fn folder(&self) -> Option<PathBuf> {
        Some(self.path.as_ref()?.parent()?.to_path_buf())
    }
}

/// Installs `disk` and sets every app setting the file holds. A value its setting refuses is
/// listed in `refused` and the default stays; a key no setting declares stays in the file.
pub fn restore(hub: &mut Hub, mut disk: Disk) {
    let rows: Vec<Setting> = hub.model.settings.rows.clone();
    let found: Vec<(Setting, Json)> = {
        let prefs = disk.prefs.lock().unwrap_or_else(PoisonError::into_inner);
        let app = rows.into_iter().filter(|row| row.store == Store::App);
        let held = |row: Setting| Some((prefs.get::<Json>(row.key.as_str())?, row));
        app.filter_map(held)
            .map(|(json, row)| (row, json))
            .collect()
    };
    for (row, json) in found {
        match read(&row, &json) {
            Ok(value) => {
                let set = SettingsMsg::Set(row.key.clone(), Some(value));
                step(&mut hub.model, Msg::Settings(set));
            }
            Err(refused) => disk.refused.push(refused),
        }
    }
    if let Some(held) = hub.panes.get_mut::<Disk>(None) {
        *held = disk;
    }
}

/// Writes what is pending, on the worker: a slider dragged while a write ran is one write.
fn flush(prefs: &Mutex<Prefs>, pending: &Mutex<Pending>) {
    let values = {
        let mut pending = pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.queued = false;
        std::mem::take(&mut pending.values)
    };
    let mut prefs = prefs.lock().unwrap_or_else(PoisonError::into_inner);
    let mut failed = None;
    for (key, value) in values {
        let wrote = match value {
            Some(json) => prefs.set(&key, &json),
            None => prefs.remove(&key),
        };
        failed = wrote.err().or(failed);
    }
    if failed.is_some() {
        pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .failed = failed;
    }
}

/// Keeps the app setting `key` as the model now holds it; Yi's settings are the server's.
/// The disk worker writes it; an error the last write met is given back here.
pub fn keep(hub: &mut Hub, key: &SettingKey) -> Result<(), PrefsError> {
    let Some(row) = hub.model.settings.rows.iter().find(|row| &row.key == key) else {
        return Ok(());
    };
    let value = row.value.as_ref().map(to_json);
    let Some(disk) = hub.panes.get::<Disk>(None) else {
        return Ok(());
    };
    let (failed, ask) = {
        let mut pending = disk.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.values.insert(key.as_str().to_owned(), value);
        let ask = !std::mem::replace(&mut pending.queued, true);
        (pending.failed.take(), ask)
    };
    if ask {
        let (prefs, pending) = (Arc::clone(&disk.prefs), Arc::clone(&disk.pending));
        let job = move || flush(&prefs, &pending);
        match hub.panes.get::<Worker>(None) {
            Some(worker) => worker.run(job),
            None => job(),
        }
    }
    failed.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::{Path, PathBuf};

    use yd_core::settings::schema::{Choice, Control, SettingValue};

    use super::*;

    type Check = Result<(), Box<dyn Error>>;

    fn file(name: &str) -> Result<PathBuf, Box<dyn Error>> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/settings-tests");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(name);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(path)
    }

    fn row(key: &str, control: Control, default: SettingValue) -> Setting {
        Setting {
            key: SettingKey::new(key),
            group: "Appearance".into(),
            name: key.to_owned().into(),
            description: "".into(),
            control,
            store: Store::App,
            default: Some(default),
            value: None,
        }
    }

    fn started() -> Result<yd_ui::win::Shared, Box<dyn Error>> {
        let hub = Hub::new(&[], Vec::new());
        let choices = vec![
            Choice {
                label: "Dense".into(),
                value: "dense".into(),
            },
            Choice {
                label: "Roomy".into(),
                value: "roomy".into(),
            },
        ];
        let glass = Control::Slider {
            min: 0.0,
            max: 1.0,
            unit: "%".into(),
        };
        let rows = vec![
            row(
                "appearance.density",
                Control::Select(choices.into()),
                SettingValue::Text("dense".into()),
            ),
            row("appearance.glass", glass, SettingValue::Number(0.2)),
        ];
        step(
            &mut hub.try_borrow_mut()?.model,
            Msg::Settings(SettingsMsg::Declare(rows)),
        );
        Ok(hub)
    }

    fn glass(hub: &yd_ui::win::Shared) -> Result<Option<SettingValue>, Box<dyn Error>> {
        Ok(hub
            .try_borrow()?
            .model
            .settings
            .value("appearance.glass")
            .cloned())
    }

    #[test]
    fn a_kept_value_is_there_after_a_restart_and_a_reset_is_forgotten() -> Check {
        let path = file("restart.json")?;
        let hub = started()?;
        let key = SettingKey::new("appearance.glass");
        {
            let mut hub = hub.try_borrow_mut()?;
            restore(&mut hub, Disk::at(&path));
            step(
                &mut hub.model,
                Msg::Settings(SettingsMsg::Set(
                    key.clone(),
                    Some(SettingValue::Number(0.6)),
                )),
            );
            keep(&mut hub, &key)?;
        }
        let second = started()?;
        restore(&mut *second.try_borrow_mut()?, Disk::at(&path));
        assert_eq!(glass(&second)?, Some(SettingValue::Number(0.6)));
        {
            let mut hub = second.try_borrow_mut()?;
            step(
                &mut hub.model,
                Msg::Settings(SettingsMsg::Set(key.clone(), None)),
            );
            keep(&mut hub, &key)?;
        }
        let third = started()?;
        restore(&mut *third.try_borrow_mut()?, Disk::at(&path));
        assert_eq!(
            glass(&third)?,
            Some(SettingValue::Number(0.2)),
            "the default is in force again"
        );
        Ok(())
    }

    #[test]
    fn a_dragged_slider_is_written_by_the_worker_as_its_last_value() -> Check {
        let path = file("dragged.json")?;
        let hub = started()?;
        let key = SettingKey::new("appearance.glass");
        let (release, held) = std::sync::mpsc::channel::<()>();
        {
            let mut hub = hub.try_borrow_mut()?;
            restore(&mut hub, Disk::at(&path));
            let worker = hub.panes.get_mut::<Worker>(None).ok_or("no worker")?;
            *worker = Worker::start()?;
            worker.run(move || {
                let _ = held.recv();
            });
            for step_of in 1..=40_u32 {
                let value = SettingValue::Number(f64::from(step_of) / 40.0);
                let set = SettingsMsg::Set(key.clone(), Some(value));
                step(&mut hub.model, Msg::Settings(set));
                keep(&mut hub, &key)?;
            }
        }
        assert!(!path.exists(), "nothing is written on the UI thread");
        release.send(())?;
        if let Some(worker) = hub.try_borrow_mut()?.panes.get_mut::<Worker>(None) {
            worker.finish();
        }
        let second = started()?;
        restore(&mut *second.try_borrow_mut()?, Disk::at(&path));
        assert_eq!(glass(&second)?, Some(SettingValue::Number(1.0)));
        Ok(())
    }

    #[test]
    fn a_bad_value_is_refused_by_name_the_default_kept_and_an_unknown_key_survives() -> Check {
        let path = file("tolerant.json")?;
        std::fs::write(
            &path,
            r#"{"appearance.glass": 5, "appearance.density": "roomy", "later.key": [1, 2]}"#,
        )?;
        let hub = started()?;
        {
            let mut hub = hub.try_borrow_mut()?;
            restore(&mut hub, Disk::at(&path));
            let held = hub.panes.get::<Disk>(None).ok_or("no store")?;
            let said: Vec<String> = held.refused.iter().map(ToString::to_string).collect();
            assert_eq!(said, ["appearance.glass takes a number from 0 to 1"]);
            let key = SettingKey::new("appearance.density");
            step(
                &mut hub.model,
                Msg::Settings(SettingsMsg::Set(
                    key.clone(),
                    Some(SettingValue::Text("dense".into())),
                )),
            );
            keep(&mut hub, &key)?;
        }
        assert_eq!(
            glass(&hub)?,
            Some(SettingValue::Number(0.2)),
            "the default stays"
        );
        let after = std::fs::read_to_string(&path)?;
        assert!(
            after.contains("later.key"),
            "a key this build does not know is not dropped: {after}"
        );
        assert!(
            after.contains(r#""dense""#),
            "and the new value is written: {after}"
        );
        Ok(())
    }
}

//! The profile-library plugin supplies catalog data; native Frame Member owns placement and edits.
use serde_json::{Value, json};
use solidcraft_engine::doc::frame;
use solidcraft_engine::plugin::ui::{PanelData, UiValue};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::dialogs_frame::Member;
use crate::selection::SelInput;
use crate::ui_kit::{Button, button, choice_row};

#[derive(Clone, Debug, Default)]
pub struct State {
    data: Option<PanelData>,
    profiles: Vec<Value>,
    requested: bool,
    refresh: bool,
    selected: Option<String>,
    matched: bool,
    dimensions: Option<(usize, Vec<String>)>,
    pub snapshot: Option<Value>,
}

fn available(app: &SolveApp) -> bool {
    app.session.plugin_state.panels.contains_key("profile-library") && !app.session.disabled_plugins.contains("profile-library")
}

/// Queue a read only once on opening, or on explicit Refresh. The normal plugin worker and
/// existing folder grants load the catalog; no file reads or guest work happen during painting.
pub(crate) fn tick(app: &mut SolveApp) {
    if !available(app) {
        return;
    }
    let Some(plugin) = app.plugin_menu.listed.iter().find(|p| p.id == "profile-library" && p.entries.iter().any(|e| e.name == "catalog")) else {
        return;
    };
    let params = json!({"path":plugin.path,"approval":plugin.approval,"entry":"catalog","params":{}});
    let Some(d) = app.dialog.as_mut() else { return };
    let Kind::Frame(m) = &mut d.kind else { return };
    if m.library.requested && !m.library.refresh {
        return;
    }
    m.library.requested = true;
    m.library.refresh = false;
    if let Err(error) = crate::plugin_menu::run_panel(app, "plugin.run".into(), params) {
        app.set_status(error, true);
    }
}

fn sync(app: &SolveApp, library: &mut State) {
    let Some(data) = app.session.plugin_state.panels.get("profile-library").and_then(|p| p.data.get("library")) else { return };
    if library.data.as_ref() == Some(data) {
        return;
    }
    library.profiles = data
        .values()
        .iter()
        .filter_map(|(path, value)| {
            let row = path.strip_prefix("/profiles/")?.strip_suffix("/definition")?;
            let index = row.parse::<usize>().ok()?;
            let UiValue::Text(text) = value else { return None };
            let record: Value = serde_json::from_str(text).ok()?;
            let name = record.get("name")?.as_str()?;
            (!name.trim().is_empty() && name.len() <= 128 && (record.get("profile").is_some() || record.get("sketch").is_some()))
                .then_some((index, record))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>()
        .into_values()
        .collect();
    library.data = Some(data.clone());
}

/// Apply a catalog snapshot to the native form only. Cancel therefore changes no design or file.
pub(crate) fn choose(m: &mut Member, inputs: &mut [SelInput], record: &Value) -> Result<(), String> {
    let name = record.get("name").and_then(Value::as_str).ok_or("profile name is missing")?;
    if let Some(profile) = record.get("profile") {
        let kinds = frame::profile_kinds();
        let (index, kind) =
            kinds.iter().enumerate().find(|(_, k)| Some(k.kind) == profile.get("kind").and_then(Value::as_str)).ok_or("unknown profile shape")?;
        let values = m.dims.get_mut(index).ok_or("unknown profile shape")?;
        for (key, value) in kind.required.iter().chain(kind.optional).zip(values.iter_mut()) {
            *value = profile.get(*key).map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).unwrap_or_default();
        }
        m.kind = index;
        m.library.snapshot = None;
        m.library.dimensions = Some((index, values.clone()));
    } else if let Some(sketch) = record.get("sketch") {
        m.library.snapshot = Some(json!({"name":name,"sketch":sketch}));
        m.library.dimensions = None;
    } else {
        return Err("profile has no section".into());
    }
    if m.name.is_empty() || m.library.selected.as_deref() == Some(m.name.as_str()) {
        m.name = name.to_owned();
    }
    m.library.selected = Some(name.to_owned());
    m.library.matched = true;
    m.whole = None;
    if let Some(section) = inputs.get_mut(1) {
        section.items.clear();
    }
    Ok(())
}

pub(crate) fn rows(app: &SolveApp, ui: &mut egui::Ui, m: &mut Member, inputs: &mut [SelInput]) {
    let installed = available(app);
    if !installed && m.library.snapshot.is_none() {
        return;
    }
    if installed {
        sync(app, &mut m.library);
    }
    if !crate::selection::items(inputs, 1).is_empty() || m.whole.is_some() {
        m.library.selected = None;
        m.library.dimensions = None;
        m.library.snapshot = None;
        m.library.matched = true;
    }
    if m.library.dimensions.as_ref().is_some_and(|(kind, dims)| *kind != m.kind || m.dims.get(m.kind) != Some(dims)) {
        m.library.selected = None;
        m.library.dimensions = None;
    }
    // Recognize an existing feature once after the catalog arrives. An explicit choice of
    // Custom dimensions must stay selected even if its values still match a saved size.
    if !m.library.matched && !m.library.profiles.is_empty() && m.library.selected.is_none() && m.library.snapshot.is_none() {
        m.library.matched = true;
        let kinds = frame::profile_kinds();
        if let Some((kind, values)) = kinds.get(m.kind).zip(m.dims.get(m.kind)) {
            let matched = m.library.profiles.iter().find(|record| {
                let profile = &record["profile"];
                profile["kind"].as_str() == Some(kind.kind)
                    && kind.required.iter().chain(kind.optional).zip(values).all(|(key, value)| {
                        let saved = profile.get(*key).map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).unwrap_or_default();
                        saved.split_whitespace().collect::<String>() == value.split_whitespace().collect::<String>()
                    })
            });
            if let Some(record) = matched {
                m.library.selected = record["name"].as_str().map(str::to_owned);
                m.library.dimensions = Some((m.kind, values.clone()));
            }
        }
    }
    let custom = solidcraft_ui_text::tr("ui.frame_library.custom");
    let current = m.library.selected.clone().or_else(|| m.library.snapshot.as_ref()?.get("name")?.as_str().map(str::to_owned));
    let mut labels = vec![custom.to_owned()];
    labels.extend(m.library.profiles.iter().map(|p| format!("{} / {}", p["category"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or(""))));
    let mut selected =
        current.as_ref().and_then(|name| m.library.profiles.iter().position(|p| p["name"].as_str() == Some(name))).map_or(0, |i| i + 1);
    if selected == 0
        && let Some(name) = current
    {
        selected = labels.len();
        labels.push(name);
    }
    let before = selected;
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    choice_row(ui, "ui.frame_library.label", "frame_library_profile", &mut selected, &refs);
    if selected != before {
        if selected == 0 {
            m.library.selected = None;
            m.library.matched = true;
            m.library.snapshot = None;
            m.library.dimensions = None;
            m.whole = None;
            if let Some(section) = inputs.get_mut(1) {
                section.items.clear();
            }
        } else if let Some(record) = selected.checked_sub(1).and_then(|i| m.library.profiles.get(i)).cloned() {
            if let Err(error) = choose(m, inputs, &record) {
                form_row(ui, "", |ui| {
                    ui.label(error);
                });
            }
        }
    }
    if installed {
        form_row(ui, "", |ui| {
            if button(ui, solidcraft_ui_text::tr("ui.frame_library.refresh"), Button::Small).clicked() {
                m.library.refresh = true;
            }
        });
    }
}

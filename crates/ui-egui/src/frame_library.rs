//! The profile-library plugin supplies catalog data; native Frame Member owns placement and edits.
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_engine::doc::frame;
use solidcraft_engine::plugin::ui::{PanelData, UiValue};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::dialogs_frame::Member;
use crate::selection::SelInput;
use crate::ui_kit::{Button, button, choice_row};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Category {
    SquareTube,
    RectTube,
    RoundTube,
    Extrusion,
    Angle,
    Channel,
    IBeam,
    FlatBar,
    RoundBar,
    Other,
}

impl Category {
    const ALL: [Self; 10] = [
        Self::SquareTube,
        Self::RectTube,
        Self::RoundTube,
        Self::Extrusion,
        Self::Angle,
        Self::Channel,
        Self::IBeam,
        Self::FlatBar,
        Self::RoundBar,
        Self::Other,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::SquareTube => "ui.frame_library.category.square_tube",
            Self::RectTube => "ui.frame_library.category.rect_tube",
            Self::RoundTube => "ui.frame_library.category.round_tube",
            Self::Extrusion => "ui.frame_library.category.extrusion",
            Self::Angle => "ui.frame_library.category.angle",
            Self::Channel => "ui.frame_library.category.channel",
            Self::IBeam => "ui.frame_library.category.i_beam",
            Self::FlatBar => "ui.frame_library.category.flat_bar",
            Self::RoundBar => "ui.frame_library.category.round_bar",
            Self::Other => "ui.frame_library.category.other",
        }
    }
}

/// Browsing is local to a placement group; the applied section remains shared by the feature.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Browser {
    category: Option<Category>,
    /// None: all series; zero: unrecognised series. Other categories ignore this filter.
    series: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Classification {
    category: Category,
    series: Option<u8>,
}

fn equal_dimensions(a: &Value, b: &Value) -> bool {
    use solidcraft_engine::doc::{DocError, expr};
    let text = |value: &Value| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
    let constant = |value: &Value| {
        expr::eval_with(&text(value), &|name| Err(DocError::Unknown(name.into())))
            .and_then(|value| value.to_kind(expr::Kind::Length))
            .ok()
            .filter(|value| value.is_finite() && *value > 0.0)
    };
    match (constant(a), constant(b)) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0),
        _ => !a.is_null() && text(a).split_whitespace().collect::<String>() == text(b).split_whitespace().collect::<String>(),
    }
}

fn classify(record: &Value) -> Classification {
    let name = record.get("name").and_then(Value::as_str).unwrap_or("");
    let category = record.get("category").and_then(Value::as_str).unwrap_or("");
    let text = format!("{name} {category}");
    if text.contains("80/20") || text.contains("80-20") || text.contains("8020") {
        // Ignore the brand's own 20 before reading the part number's prefix.
        let parts = text.replace("80/20", " ").replace("80-20", " ").replace("8020", " ");
        let numbers: Vec<_> = parts.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).collect();
        let prefix = |s: &&str| s.get(..2).and_then(|s| s.parse::<u8>().ok()).filter(|s| matches!(s, 20 | 25 | 30 | 40 | 45));
        // The explicit series precedes the section dimensions: 25-5050 is a 25-series
        // part even though its 50 mm dimensions do not start with 25.
        let explicit = parts.split(|c: char| !c.is_ascii_digit() && c != '-').find_map(|part| {
            let (series, size) = part.split_once('-')?;
            (series.len() == 2 && size.len() >= 4 && size.chars().all(|c| c.is_ascii_digit())).then(|| prefix(&series)).flatten()
        });
        let series = explicit
            .or_else(|| numbers.iter().filter(|s| s.len() >= 4).find_map(prefix))
            .or_else(|| numbers.iter().filter(|s| s.len() == 2).find_map(prefix));
        return Classification { category: Category::Extrusion, series };
    }
    let profile = &record["profile"];
    let category = match profile["kind"].as_str() {
        Some("rect_tube") => {
            if equal_dimensions(&profile["width"], &profile["height"]) {
                Category::SquareTube
            } else {
                Category::RectTube
            }
        }
        Some("round_tube") => Category::RoundTube,
        Some("angle") => Category::Angle,
        Some("channel") => Category::Channel,
        Some("i_beam") => Category::IBeam,
        Some("bar") => Category::FlatBar,
        Some("round_bar") => Category::RoundBar,
        _ => Category::Other,
    };
    Classification { category, series: None }
}

fn accepts(browser: Browser, class: Classification) -> bool {
    browser.category == Some(class.category)
        && (class.category != Category::Extrusion || browser.series.is_none_or(|s| s == class.series.unwrap_or(0)))
}

#[derive(Clone, Debug, Default)]
pub struct State {
    // Preview and command normalization clone Member. Share the immutable catalog rather
    // than copying every custom section on the window thread; refresh replaces the snapshot.
    data: Option<Arc<PanelData>>,
    profiles: Arc<Vec<Value>>,
    classes: Arc<Vec<Classification>>,
    pub browser: Browser,
    requested: bool,
    refresh: bool,
    selected: Option<String>,
    matched: bool,
    dimensions: Option<(usize, Vec<String>)>,
    pub snapshot: Option<Value>,
}

pub(crate) fn available(app: &SolveApp) -> bool {
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
    // PanelData equality includes updates waiting for the next rendered frame. Compare the
    // adopted values we actually read, or a pending catalog could be cached as an empty list.
    if library.data.as_ref().is_some_and(|cached| cached.values() == data.values()) {
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
        .collect::<Vec<_>>()
        .into();
    library.data = Some(Arc::new(data.clone()));
    library.classes = Arc::new(library.profiles.iter().map(classify).collect());
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
    let class = classify(record);
    m.library.browser = Browser { category: Some(class.category), series: class.series };
    m.library.matched = true;
    m.whole = None;
    if let Some(section) = inputs.get_mut(1) {
        section.items.clear();
    }
    Ok(())
}

fn current_name(library: &State) -> Option<&str> {
    library.selected.as_deref().or_else(|| library.snapshot.as_ref()?.get("name")?.as_str())
}

fn current_class(m: &Member) -> Classification {
    if let Some(name) = current_name(&m.library) {
        if let Some(index) = m.library.profiles.iter().position(|record| record["name"].as_str() == Some(name))
            && let Some(class) = m.library.classes.get(index)
        {
            return *class;
        }
        if m.library.snapshot.is_some() {
            return classify(&json!({"name":name}));
        }
    }
    let kinds = frame::profile_kinds();
    let mut profile = json!({});
    if let Some((kind, values)) = kinds.get(m.kind).zip(m.dims.get(m.kind)) {
        profile["kind"] = json!(kind.kind);
        for (key, value) in kind.required.iter().chain(kind.optional).zip(values) {
            profile[*key] = json!(value);
        }
    }
    classify(&json!({"profile":profile}))
}

fn custom_dimensions(m: &mut Member, inputs: &mut [SelInput]) {
    m.library.selected = None;
    m.library.matched = true;
    m.library.snapshot = None;
    m.library.dimensions = None;
    // Explicit Custom dimensions returns the picker to the editable shape's category.
    m.library.browser = Browser::default();
    m.whole = None;
    if let Some(section) = inputs.get_mut(1) {
        section.items.clear();
    }
}

/// Every displayed row keeps its original catalog index. Filtering never changes that mapping.
struct ProfileChoices {
    labels: Vec<String>,
    records: Vec<Option<usize>>,
    selected: usize,
    show_current: bool,
}

fn profile_choices(m: &Member, section_selected: bool) -> ProfileChoices {
    let mut choices = ProfileChoices {
        labels: vec![solidcraft_ui_text::tr("ui.frame_library.custom").to_owned()],
        records: vec![None],
        selected: 0,
        show_current: false,
    };
    let current = current_name(&m.library);
    let mut found = false;
    for (index, (record, class)) in m.library.profiles.iter().zip(m.library.classes.iter()).enumerate() {
        if !accepts(m.library.browser, *class) {
            continue;
        }
        let Some(name) = record["name"].as_str() else { continue };
        if current == Some(name) {
            choices.selected = choices.labels.len();
            found = true;
        }
        choices.labels.push(name.to_owned());
        choices.records.push(Some(index));
    }
    if !found && (current.is_some() || section_selected || !accepts(m.library.browser, current_class(m))) {
        choices.selected = choices.labels.len();
        // A snapshot saved in a design stays identifiable even without its catalog entry.
        let retained = current.filter(|name| {
            !section_selected
                && accepts(m.library.browser, current_class(m))
                && !m.library.profiles.iter().any(|record| record["name"].as_str() == Some(*name))
        });
        choices.labels.push(retained.unwrap_or_else(|| solidcraft_ui_text::tr("ui.frame_library.choose")).to_owned());
        choices.records.push(None);
        choices.show_current = retained.is_none();
    }
    choices
}

pub(crate) fn rows(app: &SolveApp, ui: &mut egui::Ui, m: &mut Member, inputs: &mut [SelInput]) {
    let installed = available(app);
    if !installed && m.library.snapshot.is_none() {
        return;
    }
    if installed {
        sync(app, &mut m.library);
    }
    let section_selected = !crate::selection::items(inputs, 1).is_empty() || m.whole.is_some();
    if section_selected {
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
    if m.library.browser.category.is_none() {
        let class = current_class(m);
        m.library.browser = Browser { category: Some(class.category), series: class.series };
    }
    let mut category = Category::ALL.iter().position(|category| Some(*category) == m.library.browser.category).unwrap_or(0);
    choice_row(ui, "ui.frame_library.category", "frame_library_category", &mut category, &Category::ALL.map(Category::label));
    m.library.browser.category = Category::ALL.get(category).copied();
    if m.library.browser.category == Some(Category::Extrusion) {
        let values = [None, Some(20), Some(25), Some(30), Some(40), Some(45), Some(0)];
        let mut series = values.iter().position(|series| *series == m.library.browser.series).unwrap_or(0);
        choice_row(
            ui,
            "ui.frame_library.series",
            "frame_library_series",
            &mut series,
            &["ui.frame_library.all_series", "20", "25", "30", "40", "45", "ui.frame_library.other_series"],
        );
        m.library.browser.series = values.get(series).copied().flatten();
    }
    let choices = profile_choices(m, section_selected);
    let mut selected = choices.selected;
    let refs: Vec<&str> = choices.labels.iter().map(String::as_str).collect();
    let picked = form_row(ui, "ui.frame_library.profile", |ui| crate::ui_kit::dropdown_picked(ui, "frame_library_profile", &mut selected, &refs));
    if picked {
        if selected == 0 {
            custom_dimensions(m, inputs);
        } else if let Some(record) = choices.records.get(selected).copied().flatten().and_then(|i| m.library.profiles.get(i)).cloned()
            && let Err(error) = choose(m, inputs, &record)
        {
            form_row(ui, "", |ui| {
                ui.label(error);
            });
        }
    }
    if choices.show_current && !picked {
        let current = if section_selected {
            solidcraft_ui_text::tr("ui.frame_library.selected_section")
        } else {
            current_name(&m.library).unwrap_or_else(|| solidcraft_ui_text::tr("ui.frame_library.custom"))
        };
        form_row(ui, "ui.frame_library.current", |ui| {
            ui.label(current);
        });
    }
    if installed {
        form_row(ui, "", |ui| {
            if button(ui, solidcraft_ui_text::tr("ui.frame_library.refresh"), Button::Small).clicked() {
                m.library.refresh = true;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_catalog(app: &mut SolveApp, records: &[Value]) {
        let mut data = PanelData::default();
        let values: Vec<_> = records
            .iter()
            .enumerate()
            .map(|(index, record)| solidcraft_engine::plugin::ui::Datum {
                path: format!("/profiles/{index}/definition"),
                value: UiValue::Text(record.to_string()),
            })
            .collect();
        data.set_data(&values).unwrap();
        data.merge_frame();
        app.session.plugin_state.panels.insert(
            "profile-library".into(),
            solidcraft_engine::plugin::PanelPackage {
                name: "Profile Library".into(),
                approval: "test-catalog".into(),
                declarations: serde_json::from_value(json!({"ui_version":1,"panels":[]})).unwrap(),
                data: [("library".into(), data)].into(),
                visible: Default::default(),
                visibility_revision: Default::default(),
            },
        );
    }

    #[test]
    fn catalog_clones_share_records_and_refresh_preserves_old_snapshot() {
        let mut h = crate::scenario::Harness::new();
        let original = json!({"name":"Original","profile":{"kind":"rect_tube","width":40,"height":40,"wall":3}});
        install_catalog(&mut h.app, std::slice::from_ref(&original));
        let mut library = State::default();
        sync(&h.app, &mut library);
        let old = library.clone();
        assert!(Arc::ptr_eq(library.data.as_ref().unwrap(), old.data.as_ref().unwrap()));
        assert!(Arc::ptr_eq(&library.profiles, &old.profiles));
        assert!(Arc::ptr_eq(&library.classes, &old.classes));
        sync(&h.app, &mut library);
        assert!(Arc::ptr_eq(&library.profiles, &old.profiles), "unchanged data reuses the parsed catalog");

        let replacement = json!({"name":"Replacement","profile":{"kind":"round_bar","diameter":20}});
        let data = h.app.session.plugin_state.panels.get_mut("profile-library").unwrap().data.get_mut("library").unwrap();
        data.set_data(&[solidcraft_engine::plugin::ui::Datum {
            path: "/profiles/0/definition".into(),
            value: UiValue::Text(replacement.to_string()),
        }])
        .unwrap();
        sync(&h.app, &mut library);
        assert!(Arc::ptr_eq(&library.profiles, &old.profiles), "pending data is not adopted early");
        h.app.session.plugin_state.panels.get_mut("profile-library").unwrap().data.get_mut("library").unwrap().merge_frame();
        sync(&h.app, &mut library);
        assert!(!Arc::ptr_eq(&library.profiles, &old.profiles));
        assert_eq!(library.profiles.as_slice(), &[replacement]);
        assert_eq!(old.profiles.as_slice(), std::slice::from_ref(&original));
        assert_eq!(old.data.as_ref().unwrap().values()["/profiles/0/definition"], UiValue::Text(original.to_string()));
    }

    #[test]
    fn catalog_categories_use_section_dimensions_and_part_series() {
        for (width, height) in [(json!(40), json!(40.0)), (json!("1 in"), json!("25.4 mm")), (json!("side"), json!(" side "))] {
            assert_eq!(classify(&json!({"profile":{"kind":"rect_tube","width":width,"height":height}})).category, Category::SquareTube);
        }
        assert_eq!(classify(&json!({"profile":{"kind":"rect_tube","width":"2 in","height":"1 in"}})).category, Category::RectTube);
        for (kind, category) in [
            ("round_tube", Category::RoundTube),
            ("angle", Category::Angle),
            ("channel", Category::Channel),
            ("i_beam", Category::IBeam),
            ("bar", Category::FlatBar),
            ("round_bar", Category::RoundBar),
            ("future", Category::Other),
        ] {
            assert_eq!(classify(&json!({"profile":{"kind":kind}})).category, category);
        }
        for (brand, series) in [("80/20", 20), ("80/20", 25), ("80-20", 30), ("8020", 40), ("80/20", 45)] {
            assert_eq!(
                classify(&json!({"name":format!("{brand} {series}-{series}{series}"),"sketch":{}})),
                Classification { category: Category::Extrusion, series: Some(series) }
            );
        }
        for part in ["25-5050", "25-5010", "25-2525"] {
            assert_eq!(
                classify(&json!({"name":part,"category":"80/20 profiles","sketch":{}})),
                Classification { category: Category::Extrusion, series: Some(25) }
            );
        }
        assert_eq!(
            classify(&json!({"name":"80/20 40-2080","sketch":{}})),
            Classification { category: Category::Extrusion, series: Some(40) },
            "an explicit series wins over a size prefix that resembles another series"
        );
        assert_eq!(classify(&json!({"name":"80/20 custom section","sketch":{}})), Classification { category: Category::Extrusion, series: None });
        assert_eq!(classify(&json!({"name":"Custom section","sketch":{}})).category, Category::Other);
    }

    fn paint(app: &SolveApp, ctx: &egui::Context, m: &mut Member, inputs: &mut [SelInput], events: Vec<egui::Event>) {
        crate::scenario::clear_handles();
        let input = egui::RawInput {
            time: Some(ctx.input(|i| i.time) + 1.0 / 60.0),
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| rows(app, ui, m, inputs)).textures_delta.clear();
    }

    fn pick(app: &SolveApp, ctx: &egui::Context, m: &mut Member, inputs: &mut [SelInput], row: &str, option: &str) {
        paint(app, ctx, m, inputs, vec![]);
        let at = crate::scenario::handle_at(&format!("dialog:{row}")).expect("picker trigger");
        for pressed in [true, false] {
            paint(app, ctx, m, inputs, crate::test_support::pointer(at, pressed));
        }
        paint(app, ctx, m, inputs, vec![]);
        let at = crate::scenario::handle_at(&format!("dialog:{row}:{option}")).unwrap_or_else(|| panic!("choice {row}: {option}"));
        for pressed in [true, false] {
            paint(app, ctx, m, inputs, crate::test_support::pointer(at, pressed));
        }
        paint(
            app,
            ctx,
            m,
            inputs,
            vec![egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
        );
        paint(app, ctx, m, inputs, vec![]);
    }

    #[test]
    fn category_and_series_browse_without_applying_and_filtered_profile_click_uses_catalog_index() {
        let mut h = crate::scenario::Harness::new();
        let records = [
            json!({"name":"Flat stock","profile":{"kind":"bar","width":50,"height":5}}),
            json!({"name":"80/20 40-4040","profile":{"kind":"round_bar","diameter":40}}),
            json!({"name":"Square stock","profile":{"kind":"rect_tube","width":40,"height":40,"wall":3}}),
            json!({"name":"80/20 20-2020","profile":{"kind":"round_bar","diameter":20}}),
            json!({"name":"80/20 30-3030","profile":{"kind":"round_bar","diameter":30}}),
            json!({"name":"80/20 25-5050","profile":{"kind":"bar","width":50,"height":50}}),
            json!({"name":"80/20 25-5010","profile":{"kind":"bar","width":50,"height":10}}),
        ];
        install_catalog(&mut h.app, &records);
        h.app.start("frame.member");
        let mut dialog = h.app.dialog.take().unwrap();
        let Kind::Frame(m) = &mut dialog.kind else { panic!("frame dialog") };
        let ctx = crate::fonts::test_context();
        paint(&h.app, &ctx, m, &mut dialog.inputs, vec![]);
        let initial = (m.kind, m.dims.clone(), m.library.snapshot.clone());
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.category", Category::Extrusion.label());
        assert_eq!((m.kind, m.dims.clone(), m.library.snapshot.clone()), initial, "browsing leaves the actual section alone");
        assert!(crate::scenario::handle_at("dialog:ui.frame_library.series").is_some());
        let choices = profile_choices(m, false);
        assert!(choices.show_current);
        assert_eq!(choices.labels[choices.selected], solidcraft_ui_text::tr("ui.frame_library.choose"));
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.series", "40");
        let choices = profile_choices(m, false);
        assert_eq!(choices.records.iter().flatten().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!((m.kind, m.dims.clone(), m.library.snapshot.clone()), initial, "a series filter also changes no geometry");
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.series", "25");
        let choices = profile_choices(m, false);
        assert_eq!(choices.records.iter().flatten().copied().collect::<Vec<_>>(), vec![5, 6]);
        assert_eq!((m.kind, m.dims.clone(), m.library.snapshot.clone()), initial, "25-series browsing also leaves the applied section alone");
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.profile", "80/20 25-5010");
        assert_eq!(current_name(&m.library), Some("80/20 25-5010"));
        assert_eq!(frame::profile_kinds()[m.kind].kind, "bar");
        assert_eq!(&m.dims[m.kind][..2], &["50", "10"], "the second filtered row uses its original catalog index");
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.series", "40");
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.profile", "80/20 40-4040");
        assert_eq!(current_name(&m.library), Some("80/20 40-4040"));
        assert_eq!(frame::profile_kinds()[m.kind].kind, "round_bar");
        assert_eq!(m.dims[m.kind][0], "40");
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.category", Category::FlatBar.label());
        assert!(crate::scenario::handle_at("dialog:ui.frame_library.series").is_none(), "only 80/20 has a Series row");
        assert_eq!(current_name(&m.library), Some("80/20 40-4040"), "category navigation preserves the applied profile");
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.profile", solidcraft_ui_text::tr("ui.frame_library.custom"));
        assert!(current_name(&m.library).is_none());
        assert!(m.library.snapshot.is_none());
        assert_eq!(m.dims[m.kind][0], "40", "Custom dimensions retains editable numeric values");
        let choices = profile_choices(m, false);
        assert_eq!(choices.selected, 0, "Custom dimensions remains visibly selected");
    }

    #[test]
    fn groups_restore_filters_while_the_applied_section_stays_shared() {
        let mut h = crate::scenario::Harness::new();
        let record = json!({"name":"80/20 40-4040","profile":{"kind":"bar","width":40,"height":40}});
        install_catalog(&mut h.app, std::slice::from_ref(&record));
        h.app.start("frame.member");
        let mut dialog = h.app.dialog.take().unwrap();
        let Kind::Frame(m) = &mut dialog.kind else { panic!("frame dialog") };
        sync(&h.app, &mut m.library);
        m.library.browser = Browser { category: Some(Category::SquareTube), series: None };
        m.add_group(&mut dialog.inputs);
        choose(m, &mut dialog.inputs, &record).unwrap();
        m.library.browser.series = Some(30);
        m.show(0, &mut dialog.inputs);
        assert_eq!(m.library.browser.category, Some(Category::SquareTube));
        assert_eq!(current_name(&m.library), Some("80/20 40-4040"), "group filters do not restore obsolete geometry");
        assert!(profile_choices(m, false).show_current);
        m.show(1, &mut dialog.inputs);
        assert_eq!(m.library.browser, Browser { category: Some(Category::Extrusion), series: Some(30) });
        let before = m.library.browser;
        install_catalog(&mut h.app, &[record, json!({"name":"Extra","profile":{"kind":"bar","width":60,"height":5}})]);
        sync(&h.app, &mut m.library);
        assert_eq!(m.library.browser, before, "refresh retains the active group's filters");
        assert_eq!(current_name(&m.library), Some("80/20 40-4040"));
        m.remove_group(&mut dialog.inputs);
        assert_eq!(m.library.browser.category, Some(Category::SquareTube), "removing a group restores its neighbour's filters");
    }

    #[test]
    fn saved_snapshot_without_catalog_is_identified_and_can_return_to_custom_dimensions() {
        let mut h = crate::scenario::Harness::new();
        h.app.start("frame.member");
        let mut dialog = h.app.dialog.take().unwrap();
        let Kind::Frame(m) = &mut dialog.kind else { panic!("frame dialog") };
        m.library.snapshot = Some(json!({"name":"80-20 30-3030","sketch":{}}));
        let ctx = crate::fonts::test_context();
        paint(&h.app, &ctx, m, &mut dialog.inputs, vec![]);
        assert_eq!(m.library.browser, Browser { category: Some(Category::Extrusion), series: Some(30) });
        let choices = profile_choices(m, false);
        assert_eq!(choices.labels[choices.selected], "80-20 30-3030");
        assert!(!choices.show_current, "the retained snapshot has an accurate selected label");
        pick(&h.app, &ctx, m, &mut dialog.inputs, "ui.frame_library.profile", solidcraft_ui_text::tr("ui.frame_library.custom"));
        assert!(m.library.snapshot.is_none());
    }

    /// Local diagnostic only: provide a JSON array of catalog records. No timing assertions.
    #[test]
    #[ignore = "requires SOLIDCRAFT_BENCH_CATALOG pointing to a local profile catalog"]
    fn catalog_dialog_clone_benchmark() {
        let path = std::env::var("SOLIDCRAFT_BENCH_CATALOG").unwrap();
        let records: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut h = crate::scenario::Harness::new();
        install_catalog(&mut h.app, &records);
        h.app.start("frame.member");
        let mut dialog = h.app.dialog.take().unwrap();
        let Kind::Frame(member) = &mut dialog.kind else { panic!("frame dialog") };
        sync(&h.app, &mut member.library);
        assert_eq!(member.library.profiles.len(), records.len());
        for _ in 0..10 {
            std::hint::black_box(dialog.clone());
        }
        let start = std::time::Instant::now();
        for _ in 0..500 {
            std::hint::black_box(dialog.clone());
        }
        println!("catalog={} dialog_clone_drop_500_us={}", records.len(), start.elapsed().as_micros());
        let expected = crate::dialogs::apply_commands(&h.app, &dialog);
        let start = std::time::Instant::now();
        for _ in 0..100 {
            assert_eq!(std::hint::black_box(crate::dialogs::apply_commands(&h.app, &dialog)), expected);
        }
        println!("apply_commands_100_us={}", start.elapsed().as_micros());
    }
}

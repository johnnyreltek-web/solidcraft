//! Cursor quick menu: S opens a saved command list for Sketch, Part or Assembly. Its editor
//! adds, removes and reorders commands independently for each context. The shared keymap owns
//! the shortcut. Drawing and Render retain command search.
//!
//! The optional menu after selection keeps its selection-aware commands and delay. Both menus
//! use the existing context-menu host, command dispatch, keyboard navigation and dismissal.

use egui::{Pos2, Rect};
use serde_json::Value;
use solidcraft_engine::Sel;
use std::collections::BTreeMap;

use crate::SolveApp;
use crate::context_menu::Target;
use crate::ui_kit::Item;

const MAX_TOOLS: usize = 24;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Context {
    Sketch,
    #[default]
    Part,
    Assembly,
}

impl Context {
    const ALL: [Self; 3] = [Self::Sketch, Self::Part, Self::Assembly];

    fn of(app: &SolveApp) -> Self {
        if crate::sketch_palette::editing(app) {
            Self::Sketch
        } else if app.session.doc.active_occurrences().any(|o| o.parent == app.session.active_component)
            || (app.session.active_component == 0 && app.ui.tab == "ASSEMBLE")
        {
            Self::Assembly
        } else {
            Self::Part
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Sketch => "sketch",
            Self::Part => "part",
            Self::Assembly => "assembly",
        }
    }

    fn label(self) -> &'static str {
        solidcraft_ui_text::tr(match self {
            Self::Sketch => "quick_menu.sketch",
            Self::Part => "quick_menu.part",
            Self::Assembly => "quick_menu.assembly",
        })
    }

    fn defaults(self) -> &'static [&'static str] {
        match self {
            Self::Sketch => &[
                "sketch.line",
                "sketch.rectangle.two_point",
                "sketch.circle.center",
                "sketch.arc.three_point",
                "sketch.dimension",
                "sketch.trim",
                "sketch.offset",
                "sketch.finish",
            ],
            Self::Part => {
                &["sketch.create", "solid.extrude", "solid.revolve", "solid.hole", "solid.fillet", "solid.chamfer", "solid.shell", "solid.move"]
            }
            Self::Assembly => &[
                "component.insert_design",
                "component.create",
                "joint.create",
                "joint.as_built",
                "occurrence.move",
                "occurrence.ground",
                "joint.drive",
                "assembly.bom",
            ],
        }
    }
}

/// Only command lists persist. Transient windows and pending input never reopen on launch.
#[derive(Default)]
pub(crate) struct Config {
    custom: BTreeMap<String, Vec<String>>,
    pub(crate) editor: bool,
    editor_owner: u32,
    context: Context,
    filter: String,
    pending: bool,
    manual: bool,
    opened: Option<(u64, u64, Context)>,
    owner: Option<u32>,
}

fn allowed(id: &str) -> bool {
    !matches!(id, "ui.quick_menu" | "ui.palette")
        && !crate::workspace::AGENT_ONLY.contains(&id)
        && solidcraft_engine::find_command(id).is_some_and(|c| c.planned.is_none())
}

impl Config {
    pub(crate) fn prefs(&self) -> Value {
        serde_json::json!(self.custom)
    }

    pub(crate) fn load(&mut self, value: &Value) {
        self.custom.clear();
        for context in Context::ALL {
            if let Some(rows) = value.get(context.key()).and_then(Value::as_array) {
                let mut list = Vec::new();
                for id in rows.iter().take(256).filter_map(Value::as_str).filter(|id| id.len() <= 256 && allowed(id)) {
                    if !list.iter().any(|old| old == id) && list.len() < MAX_TOOLS {
                        list.push(id.to_owned());
                    }
                }
                self.custom.insert(context.key().into(), list);
            }
        }
    }

    fn tools(&self, context: Context) -> Vec<String> {
        self.custom.get(context.key()).cloned().unwrap_or_else(|| context.defaults().iter().map(|s| (*s).into()).collect())
    }
}

pub(crate) fn request(app: &mut SolveApp) {
    if crate::drawing_workspace::active(app) || crate::render_workspace::active(app) {
        crate::shortcut_box::open(app, None);
    } else if owns_window(app) && app.viewport.context_menu.is_some() && app.menu.target == Target::Quick {
        crate::context_menu::close(app);
    } else {
        app.ui.palette_open = false;
        app.quick_menu.owner = Some(app.view_windows.current_window());
        app.quick_menu.pending = true;
    }
}

pub(crate) fn customize(app: &mut SolveApp) {
    app.quick_menu.editor_owner = app.view_windows.current_window();
    app.quick_menu.context = Context::of(app);
    app.quick_menu.filter.clear();
    app.quick_menu.editor = true;
}

pub(crate) fn editor_in_window(app: &SolveApp) -> bool {
    app.quick_menu.editor && app.quick_menu.editor_owner == app.view_windows.current_window()
}

pub(crate) fn owns_window(app: &SolveApp) -> bool {
    app.quick_menu.owner.is_none_or(|owner| owner == app.view_windows.current_window())
}

fn custom_items(app: &SolveApp) -> Vec<Item> {
    let context = Context::of(app);
    let mut rows = vec![Item::heading(context.label())];
    for id in app.quick_menu.tools(context) {
        if let Some(spec) = solidcraft_engine::find_command(&id) {
            let info = crate::render_workspace::interactive_command_info(app, spec);
            let mut row = crate::context_menu::cmd(app, &id, solidcraft_ui_text::tr_command(spec.id, info.label));
            row.enabled = info.enabled;
            row.icon = info.icon.into();
            rows.push(row);
        }
    }
    rows.push(Item::sep());
    rows.push(Item::action("ui.palette", solidcraft_ui_text::tr_command("ui.palette", "Search Commands"), "zoom"));
    rows.push(Item::action("ui.quickMenuCustomize", solidcraft_ui_text::tr("quick_menu.customize"), "settings"));
    rows
}

/// Standalone editor: changes apply immediately and save with the ordinary application settings.
pub(crate) fn editor(app: &mut SolveApp, ctx: &egui::Context) {
    if !editor_in_window(app) {
        return;
    }
    let mut context = app.quick_menu.context;
    let mut filter = app.quick_menu.filter.clone();
    let catalog: Vec<_> = solidcraft_engine::command_specs()
        .into_iter()
        .filter(|c| allowed(c.id) && crate::shortcut_box::listed(app, c))
        .map(|c| (c.id, solidcraft_ui_text::tr_command(c.id, c.label)))
        .collect();
    let mut catalog = catalog;
    catalog.sort_by(|a, b| a.1.cmp(b.1));
    let (closed, action) =
        crate::ui_kit::Window::new(egui::Id::unique("sc_quick_menu_editor"), "settings", solidcraft_ui_text::tr("quick_menu.customize"))
            .cards(2.0)
            .fields()
            .actions("", "Close", None)
            .show(ctx, true, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for choice in Context::ALL {
                        let r = ui.selectable_label(context == choice, choice.label());
                        crate::scenario::publish_target(&format!("quick_menu:context:{}", choice.key()), r.rect);
                        if r.clicked() {
                            context = choice;
                        }
                    }
                });
                ui.label(solidcraft_ui_text::tr("quick_menu.saved"));
                let mut list = app.quick_menu.tools(context);
                let mut changed = false;
                let mut action = None;
                egui::ScrollArea::vertical().id_salt("quick_menu_chosen").max_height(220.0).show(ui, |ui| {
                    for (index, id) in list.iter().enumerate() {
                        ui.push_id(id, |ui| {
                            ui.horizontal(|ui| {
                                let label =
                                    solidcraft_engine::find_command(id).map_or(id.as_str(), |c| solidcraft_ui_text::tr_command(c.id, c.label));
                                ui.label(label);
                                for (op, text, enabled) in
                                    [(0, "quick_menu.up", index > 0), (1, "quick_menu.down", index + 1 < list.len()), (2, "quick_menu.remove", true)]
                                {
                                    let r = crate::ui_kit::button(ui, solidcraft_ui_text::tr(text), crate::ui_kit::Button::Enabled(enabled));
                                    crate::scenario::publish_target(&format!("quick_menu:{op}:{index}"), r.rect);
                                    if r.clicked() {
                                        action = Some((op, index));
                                    }
                                }
                            });
                        });
                    }
                });
                if let Some((op, index)) = action {
                    match op {
                        0 if index > 0 && index < list.len() => {
                            list.swap(index, index - 1);
                            changed = true;
                        }
                        1 if index + 1 < list.len() => {
                            list.swap(index, index + 1);
                            changed = true;
                        }
                        2 if index < list.len() => {
                            list.remove(index);
                            changed = true;
                        }
                        _ => {}
                    }
                }
                let reset = crate::ui_kit::button(ui, solidcraft_ui_text::tr("quick_menu.reset"), crate::ui_kit::Button::Normal);
                crate::scenario::publish_target("quick_menu:reset", reset.rect);
                if reset.clicked() {
                    app.quick_menu.custom.remove(context.key());
                    list = app.quick_menu.tools(context);
                    changed = false;
                }
                ui.separator();
                let (_, r) = crate::ui_kit::text_field_reserving(ui, &mut filter, solidcraft_ui_text::tr("quick_menu.filter"), 0.0);
                crate::scenario::publish_target("quick_menu:filter", r.rect);
                let query = filter.trim().to_lowercase();
                egui::ScrollArea::vertical().id_salt("quick_menu_catalog").max_height(180.0).show(ui, |ui| {
                    for (id, label) in &catalog {
                        if !query.is_empty() && !label.to_lowercase().contains(&query) && !id.contains(&query) {
                            continue;
                        }
                        let enabled = list.len() < MAX_TOOLS && !list.iter().any(|s| s == id);
                        let r = crate::ui_kit::button(ui, *label, crate::ui_kit::Button::Enabled(enabled));
                        crate::scenario::publish_target(&format!("quick_menu:add:{id}"), r.rect);
                        if r.clicked() {
                            list.push((*id).into());
                            changed = true;
                        }
                    }
                });
                if changed {
                    app.quick_menu.custom.insert(context.key().into(), list);
                }
            });
    app.quick_menu.context = context;
    app.quick_menu.filter = filter.chars().take(256).collect();
    app.quick_menu.editor = !closed && action != crate::ui_kit::Action::Cancel;
}
/// How far the pointer may move away from the open menu before it closes, in points.
pub(crate) const LEAVE: f32 = 60.0;
/// How long after a selecting click the menu waits before it opens by itself, in seconds.
const DELAY: f64 = 0.3;
/// How far a click may move and still count as a click (not a box selection), in points.
const CLICK: f32 = 6.0;

/// What the quick menu keeps between frames.
#[derive(Default)]
pub struct Watch {
    /// The selection last seen: a different one is a change.
    seen: Vec<Sel>,
    /// Where the primary button went down in the viewport, while it is down.
    press: Option<Pos2>,
    /// The last viewport click that did not move (where and when it was let go).
    click: Option<(Pos2, f64)>,
    /// A selection made by a click: the menu opens at this point at this time.
    due: Option<(Pos2, f64)>,
}

/// The commands that take the session's selection, in toolbar order: constraints with the
/// parameters it gives them, other commands to start as from the toolbar.
pub(crate) fn fitting(app: &SolveApp) -> Vec<Item> {
    if !app.session.has_selection() {
        return Vec::new();
    }
    solidcraft_engine::command_specs()
        .into_iter()
        .filter(|spec| !spec.tab.is_empty() && spec.planned.is_none())
        .filter_map(|spec| {
            let params = spec.for_selection(&app.session)?;
            let item = crate::context_menu::cmd(app, spec.id, spec.label);
            Some(if spec.panel == "CONSTRAINTS" { item.with(params) } else { item })
        })
        .collect()
}

/// The menu's rows: the commands that take the selection, else the studio's tools.
pub(crate) fn items(app: &SolveApp) -> Vec<Item> {
    if app.quick_menu.manual {
        return custom_items(app);
    }
    let fit = fitting(app);
    if !fit.is_empty() {
        return fit;
    }
    crate::mouse_gesture::items(app, crate::mouse_gesture::Studio::of(app))
}

/// Opens the quick menu at `at`.
pub fn open(app: &mut SolveApp, at: Pos2) {
    app.quick_menu.manual = false;
    app.quick_menu.owner = None;
    crate::context_menu::open_for(app, at, Target::Quick);
}

/// Whether the pointer has left the open quick menu `rect` far enough to close it.
pub(crate) fn left(rect: Rect, pointer: Option<Pos2>) -> bool {
    pointer.is_some_and(|p| !rect.expand(LEAVE).contains(p))
}

/// One frame: follows clicks in the viewport and the selection, opens the menu after a
/// selecting click (with the preference on) and closes it when the selection changes.
pub(crate) fn watch(app: &mut SolveApp, ctx: &egui::Context) {
    if !owns_window(app)
        && (app.quick_menu.pending || (app.quick_menu.manual && app.viewport.context_menu.is_some() && app.menu.target == Target::Quick))
    {
        return;
    }
    if app.quick_menu.pending {
        app.quick_menu.pending = false;
        let at = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(ctx.content_rect().center());
        app.menu.quick.seen = app.session.selection.clone();
        crate::context_menu::open_for(app, at, Target::Quick);
        app.quick_menu.manual = true;
        app.quick_menu.opened = Some((app.session.uid, app.session.active_component, Context::of(app)));
    }
    if app.quick_menu.manual
        && app.viewport.context_menu.is_some()
        && app.menu.target == Target::Quick
        && app.quick_menu.opened != Some((app.session.uid, app.session.active_component, Context::of(app)))
    {
        crate::context_menu::close(app);
    }
    let (pressed, released, pos, now) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.latest_pos(), i.time));
    if pressed {
        app.menu.quick.press = pos.filter(|_| app.viewport.mouse.is_some());
    }
    if released
        && let (Some(p0), Some(p)) = (app.menu.quick.press.take(), pos)
        && p0.distance(p) <= CLICK
    {
        app.menu.quick.click = Some((p, now));
    }
    if app.session.selection != app.menu.quick.seen {
        app.menu.quick.seen = app.session.selection.clone();
        app.menu.quick.due = None;
        if app.viewport.context_menu.is_some() && app.menu.target == Target::Quick {
            crate::context_menu::close(app);
        }
        // Only a click's own selection: not a box selection, a browser pick or a script's.
        // The click is used up here, so a later change of the selection can't take it again.
        if let Some((p, t)) = app.menu.quick.click.take()
            && now - t < 0.5
            && after_selection_ready(app)
        {
            app.menu.quick.due = Some((p, now + DELAY));
        }
    }
    let Some((p, t)) = app.menu.quick.due else { return };
    if !after_selection_ready(app) || pos.is_some_and(|q| q.distance(p) > CLICK) {
        app.menu.quick.due = None;
    } else if now >= t {
        app.menu.quick.due = None;
        if !fitting(app).is_empty() {
            open(app, p);
        }
    } else {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((t - now).max(0.01)));
    }
}

/// The preference is on and nothing else has the pointer: no menu, tool or dialog.
fn after_selection_ready(app: &SolveApp) -> bool {
    app.preferences.quick_menu_after_selection
        && !editor_in_window(app)
        && app.viewport.context_menu.is_none()
        && app.tool.is_none()
        && app.dialog.is_none()
}

#[cfg(test)]
#[path = "quick_menu_tests.rs"]
mod tests;

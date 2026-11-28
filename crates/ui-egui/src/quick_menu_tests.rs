use serde_json::json;
use solidcraft_engine::Sel;

use super::*;
use crate::context_menu::{Target, items as menu_items};

fn curve(id: &str) -> Sel {
    Sel::SketchCurve { id: id.into(), sketch: None }
}

/// A harness in a sketch with two lines (`l1`, `l2`) and two circles (`c1`, `c2`).
fn sketch() -> crate::scenario::Harness {
    let mut h = crate::scenario::Harness::new();
    h.app.home.open = false;
    h.app.run("sketch.create", json!({"plane": "XY"})).unwrap();
    h.app.run("sketch.line", json!({"points": [[0, 0], [30, 0]], "ids": ["l1"], "connect": false, "infer": false})).unwrap();
    h.app.run("sketch.line", json!({"points": [[0, 10], [30, 14]], "ids": ["l2"], "connect": false, "infer": false})).unwrap();
    h.app.run("sketch.circle.center", json!({"center": [60, 0], "radius": 5, "id": "c1"})).unwrap();
    h.app.run("sketch.circle.center", json!({"center": [90, 0], "radius": 8, "id": "c2"})).unwrap();
    h
}

fn ids(items: &[Item]) -> Vec<&str> {
    items.iter().filter(|i| i.id != "-" && !i.id.starts_with('#')).map(|i| i.id.as_str()).collect()
}

/// The commands the selection fits come from their own specs, with only the constraints whose
/// kinds suit it: two lines, a line and a circle, two circles.
#[test]
fn the_quick_menu_lists_what_the_selection_fits() {
    let mut h = sketch();
    h.app.session.selection = vec![curve("l2"), curve("l1")];
    let two_lines = fitting(&h.app);
    for id in ["sketch.constraint.parallel", "sketch.constraint.perpendicular", "sketch.constraint.collinear", "sketch.constraint.equal"] {
        let item = two_lines.iter().find(|i| i.id == id).unwrap_or_else(|| panic!("{id}: {:?}", ids(&two_lines)));
        assert_eq!(item.params, Some(json!({"a": "l2", "b": "l1"})), "{id} runs on the picks in order");
    }
    for id in ["sketch.constraint.tangent", "sketch.constraint.concentric", "solid.fillet"] {
        assert!(!ids(&two_lines).contains(&id), "{id} doesn't take two lines: {:?}", ids(&two_lines));
    }
    h.app.session.selection = vec![curve("l1"), curve("c1")];
    let line_circle = ids(&fitting(&h.app)).iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(line_circle.contains(&"sketch.constraint.tangent".to_string()), "{line_circle:?}");
    assert!(!line_circle.contains(&"sketch.constraint.parallel".to_string()), "{line_circle:?}");
    h.app.session.selection = vec![curve("c1"), curve("c2")];
    let circles = fitting(&h.app);
    for id in ["sketch.constraint.concentric", "sketch.constraint.equal", "sketch.constraint.tangent"] {
        assert!(ids(&circles).contains(&id), "{id}: {:?}", ids(&circles));
    }
    // Nothing selected: the sketch's tools, as on the gesture ring.
    h.app.session.selection.clear();
    assert!(fitting(&h.app).is_empty());
    assert!(ids(&items(&h.app)).contains(&"sketch.line"), "{:?}", ids(&items(&h.app)));
}

/// A model edge offers Fillet and Chamfer, started as from the toolbar so their dialog takes the
/// edge; faces offer Shell; two edges of different bodies offer no blend.
#[test]
fn model_picks_offer_their_commands_to_start() {
    let mut h = crate::scenario::Harness::new();
    h.app.home.open = false;
    h.app.run("solid.box", json!({"length": 10, "width": 20, "height": 30})).unwrap();
    let body = h.app.session.world_state().unwrap().bodies.first().map(|b| b.name.clone()).unwrap();
    let edge = |x: f64| Sel::Edge { body: body.clone(), index: 0, point: solidcore_record::Vec3::new(x, 0.0, 0.0) };
    h.app.session.selection = vec![edge(5.0)];
    let fit = fitting(&h.app);
    for id in ["solid.fillet", "solid.chamfer"] {
        let item = fit.iter().find(|i| i.id == id).unwrap_or_else(|| panic!("{id}: {:?}", ids(&fit)));
        assert!(item.params.is_none(), "{id} starts its dialog, which takes the edge");
    }
    assert!(!ids(&fit).iter().any(|id| id.starts_with("sketch.constraint.")), "{:?}", ids(&fit));
    h.app.session.selection = vec![Sel::Face { body: body.clone(), index: 0, point: solidcore_record::Vec3::new(5.0, 10.0, 30.0) }];
    assert!(ids(&fitting(&h.app)).contains(&"solid.shell"), "{:?}", ids(&fitting(&h.app)));
    h.app.session.selection = vec![edge(5.0), Sel::Edge { body: "Other".into(), index: 1, point: solidcore_record::Vec3::new(0.0, 0.0, 0.0) }];
    assert!(!ids(&fitting(&h.app)).contains(&"solid.fillet"));
}

/// S opens the quick menu at the cursor; changing the selection closes it.
#[test]
fn the_key_opens_the_quick_menu_and_a_new_selection_closes_it() {
    let mut h = sketch();
    h.app.session.selection = vec![curve("l2"), curve("l1")];
    h.frames(3);
    h.step(&json!({"key": "S"})).unwrap();
    h.frames(2);
    assert!(h.app.viewport.context_menu.is_some(), "S opens the menu");
    assert_eq!(h.app.menu.target, Target::Quick);
    let listed = h.call("ui.menu", json!({}));
    let labels: Vec<&str> = listed["result"]["items"].as_array().into_iter().flatten().filter_map(|i| i["label"].as_str()).collect();
    assert!(labels.contains(&"Line") && labels.contains(&"Finish Sketch"), "{listed}");
    assert!(!labels.contains(&"Parallel"), "the manual menu keeps the chosen tools: {listed}");
    let _ = h.app.run("select.set", json!({"items": [{"type": "sketch_curve", "id": "c1"}]}));
    h.frames(2);
    assert!(h.app.viewport.context_menu.is_none(), "a new selection closes it");
}

/// The menu closes once the pointer is [`LEAVE`] points clear of it, not before.
#[test]
fn the_menu_closes_when_the_pointer_moves_away() {
    let rect = Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(250.0, 200.0));
    assert!(!left(rect, Some(egui::pos2(120.0, 120.0))));
    assert!(!left(rect, Some(egui::pos2(100.0 - LEAVE + 1.0, 150.0))));
    assert!(left(rect, Some(egui::pos2(100.0 - LEAVE - 1.0, 150.0))));
    assert!(!left(rect, None), "no pointer: it stays");
}

/// Off by default; with the preference off, selecting never opens the menu.
#[test]
fn opening_after_a_selection_is_off_by_default() {
    assert!(!crate::prefs::Prefs::default().quick_menu_after_selection);
    let mut h = sketch();
    h.app.menu.quick.click = Some((egui::pos2(200.0, 200.0), 0.0));
    h.app.session.selection = vec![curve("l2"), curve("l1")];
    h.frames(30);
    assert!(h.app.viewport.context_menu.is_none());
}

/// With the preference on, a click's selection that a command takes opens the menu where the
/// click was, after a short pause; a selection nothing takes opens nothing.
#[test]
fn with_the_preference_a_selecting_click_opens_the_menu() {
    let mut h = sketch();
    h.app.preferences.quick_menu_after_selection = true;
    h.frames(2);
    let at = egui::pos2(400.0, 300.0);
    h.app.synthetic.push(egui::Event::PointerMoved(at));
    h.frames(1);
    h.app.menu.quick.click = Some((at, h.ctx.input(|i| i.time)));
    h.app.session.selection = vec![curve("l2"), curve("l1")];
    for _ in 0..40 {
        h.frames(1);
        if h.app.viewport.context_menu.is_some() {
            break;
        }
    }
    assert_eq!(h.app.viewport.context_menu, Some(at), "it opens where the click was");
    assert_eq!(h.app.menu.target, Target::Quick);
    assert!(ids(&menu_items(&h.app, &Target::Quick)).contains(&"sketch.constraint.parallel"));
}

/// The right-click menu's constraints come from the same specs: what it offered before for two
/// lines, two circles, a line and a circle, still offered, first and in the order it had.
#[test]
fn the_right_click_constraints_come_from_the_command_specs() {
    let mut h = sketch();
    let cases: [(&[&str], &[&str]); 3] = [
        (
            &["l1", "l2"],
            &[
                "sketch.constraint.parallel",
                "sketch.constraint.perpendicular",
                "sketch.constraint.collinear",
                "sketch.constraint.equal",
                "sketch.constraint.midpoint",
            ],
        ),
        (&["c1", "c2"], &["sketch.constraint.concentric", "sketch.constraint.equal", "sketch.constraint.tangent"]),
        (&["l1", "c1"], &["sketch.constraint.tangent"]),
    ];
    for (picks, expected) in cases {
        h.app.session.selection = picks.iter().map(|id| curve(id)).collect();
        let menu = menu_items(&h.app, &Target::Viewport);
        let listed = ids(&menu);
        for id in expected {
            assert!(listed.contains(id), "{picks:?}: {id} missing from {listed:?}");
        }
        assert!(!listed.contains(&"sketch.constraint.fix"), "Fix/Unfix keeps its own row");
        let heading = menu.iter().position(|i| i.is_heading() && i.label == "Constraints").expect("a Constraints heading");
        let constraints: Vec<&str> = ids(&menu[heading + 1..]);
        assert_eq!(&constraints[..expected.len()], expected, "{picks:?}: the rows it had come first, in their old order");
    }
}

/// A box that takes two lines and a constraint icon still offers what the two lines take: the
/// rules read only the selection's sketch geometry.
#[test]
fn constraint_icons_in_the_selection_do_not_hide_the_lines_constraints() {
    let mut h = sketch();
    h.app.run("sketch.constraint.horizontal_vertical", json!({"line": "l1", "mode": "horizontal"})).unwrap();
    let st = h.app.session.world_state().unwrap();
    let icon = st.sketch(h.app.session.active_sketch.unwrap()).unwrap().sketch.constraints.last().map(|c| c.id.clone()).unwrap();
    h.app.session.selection = vec![curve("l1"), curve("l2"), Sel::SketchConstraint { id: icon }];
    let listed = ids(&menu_items(&h.app, &Target::Viewport)).iter().map(|s| s.to_string()).collect::<Vec<_>>();
    for id in ["sketch.constraint.parallel", "sketch.constraint.perpendicular", "sketch.constraint.equal"] {
        assert!(listed.contains(&id.to_string()), "{id} missing from {listed:?}");
    }
}

/// A sketch takes precedence; root assemblies and subassemblies differ from editing a leaf.
#[test]
fn quick_menu_context_tracks_sketches_and_active_assembly_components() {
    let mut h = crate::scenario::Harness::new();
    h.app.home.open = false;
    assert_eq!(Context::of(&h.app), Context::Part);
    h.app.ui.tab = "ASSEMBLE".into();
    assert_eq!(Context::of(&h.app), Context::Assembly, "an empty assembly can begin from its ribbon");
    h.app.ui.tab = "MODEL".into();
    let sub = h.app.run("component.create", json!({"name":"Subassembly"})).unwrap()["component"].as_u64().unwrap();
    let leaf = h.app.run("component.create", json!({"name":"Part"})).unwrap()["component"].as_u64().unwrap();
    assert_eq!(Context::of(&h.app), Context::Part, "creating a leaf activates that part");
    h.app.run("component.activate", json!({"component":sub})).unwrap();
    assert_eq!(Context::of(&h.app), Context::Assembly);
    h.app.run("component.activate", json!({"component":0})).unwrap();
    assert_eq!(Context::of(&h.app), Context::Assembly);
    h.app.run("component.activate", json!({"component":leaf})).unwrap();
    h.app.ui.tab = "ASSEMBLE".into();
    assert_eq!(Context::of(&h.app), Context::Part, "a leaf remains a part even with the old assembly tab selected");
    h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
    assert_eq!(Context::of(&h.app), Context::Sketch);
    h.app.run("sketch.finish", json!({})).unwrap();
    assert_eq!(Context::of(&h.app), Context::Part);
}

#[test]
fn quick_menu_customization_is_reachable_and_edits_each_context_independently() {
    let mut h = crate::scenario::Harness::new();
    h.app.home.open = false;
    h.frames(3);
    h.step(&json!({"move":[600,400]})).unwrap();
    h.step(&json!({"key":"S"})).unwrap();
    h.frames(2);
    assert_eq!(h.call("ui.menuPick", json!({"item":"ui.quickMenuCustomize"}))["ok"], true);
    h.frames(3);
    assert!(h.app.quick_menu.editor);
    assert!(h.app.viewport.context_menu.is_none());
    let before_design = h.app.session.doc.to_json();
    let before_undo = h.app.session.undo.len();
    let mut changed = BTreeMap::new();
    for context in Context::ALL {
        h.step(&json!({"click":{"handle":format!("quick_menu:context:{}", context.key())}})).unwrap();
        h.frames(2);
        let original = h.app.quick_menu.tools(context);
        assert!(original.len() >= 2);
        h.step(&json!({"click":{"handle":"quick_menu:1:0"}})).unwrap();
        h.frames(2);
        let reordered = h.app.quick_menu.tools(context);
        assert_eq!(reordered[0], original[1]);
        assert_eq!(reordered[1], original[0]);
        h.step(&json!({"click":{"handle":"quick_menu:0:1"}})).unwrap();
        h.frames(2);
        assert_eq!(h.app.quick_menu.tools(context), original, "Up reverses Down");
        h.step(&json!({"click":{"handle":"quick_menu:2:0"}})).unwrap();
        h.frames(2);
        assert!(!h.app.quick_menu.tools(context).contains(&original[0]));
        h.step(&json!({"click":{"handle":"quick_menu:filter"}})).unwrap();
        h.step(&json!({"key":"A","ctrl":true})).unwrap();
        h.step(&json!({"text":original[0]})).unwrap();
        h.frames(2);
        h.step(&json!({"click":{"handle":format!("quick_menu:add:{}", original[0])}})).unwrap();
        h.frames(2);
        let list = h.app.quick_menu.tools(context);
        assert_eq!(list.last(), original.first());
        assert_eq!(list.len(), original.len());
        changed.insert(context.key().to_string(), list);
        for (key, list) in &changed {
            assert_eq!(h.app.quick_menu.custom.get(key), Some(list), "editing one context preserves the others");
        }
    }
    let saved = h.app.prefs();
    let mut restored = crate::SolveApp::new(solidcraft_engine::Session::default(), crate::Services::default());
    restored.load_prefs(&saved);
    assert_eq!(restored.quick_menu.custom, changed);
    assert!(!restored.quick_menu.editor, "the editor does not reopen after restart");
    h.step(&json!({"click":{"handle":"quick_menu:context:part"}})).unwrap();
    h.frames(2);
    h.step(&json!({"click":{"handle":"quick_menu:reset"}})).unwrap();
    h.frames(2);
    assert_eq!(h.app.quick_menu.tools(Context::Part), Context::Part.defaults());
    assert_eq!(h.app.quick_menu.custom.get("sketch"), changed.get("sketch"));
    assert_eq!(h.app.quick_menu.custom.get("assembly"), changed.get("assembly"));
    assert_eq!(h.app.session.doc.to_json(), before_design);
    assert_eq!(h.app.session.undo.len(), before_undo);
}

#[test]
fn quick_menu_loading_bounds_and_deduplicates_commands_and_preserves_an_empty_menu() {
    let mut config = Config::default();
    config.load(&json!({"sketch":["sketch.line","sketch.line",null,"missing.command","ui.quick_menu","ui.palette"],"part":[]}));
    assert_eq!(config.tools(Context::Sketch), ["sketch.line"]);
    assert!(config.tools(Context::Part).is_empty(), "an intentionally empty menu stays empty");
    assert!(config.tools(Context::Assembly).contains(&"joint.create".to_string()));
    let saved = config.prefs();
    let mut restored = Config::default();
    restored.load(&saved);
    assert_eq!(restored.prefs(), saved);
    let commands: Vec<_> = solidcraft_engine::command_specs().into_iter().filter(|c| allowed(c.id)).map(|c| c.id).collect();
    restored.load(&json!({"part":commands}));
    assert_eq!(restored.tools(Context::Part).len(), MAX_TOOLS);
}

#[test]
fn escape_closes_the_quick_menu_without_cancelling_the_sketch_tool_or_selection() {
    let mut h = sketch();
    h.app.start("sketch.line");
    h.app.session.selection = vec![curve("l1")];
    h.frames(3);
    h.step(&json!({"move":[600,400]})).unwrap();
    let selection = h.app.session.selection.clone();
    let active_sketch = h.app.session.active_sketch;
    let before = h.app.session.doc.to_json();
    assert_eq!(h.app.tool.as_ref().map(|tool| tool.cmd), Some("sketch.line"));
    h.step(&json!({"key":"S"})).unwrap();
    h.frames(2);
    assert!(h.app.viewport.context_menu.is_some());
    assert_eq!(h.app.menu.target, Target::Quick);
    h.step(&json!({"key":"Escape"})).unwrap();
    h.frames(2);
    assert!(h.app.viewport.context_menu.is_none());
    assert_eq!(h.app.tool.as_ref().map(|tool| tool.cmd), Some("sketch.line"));
    assert_eq!(h.app.session.active_sketch, active_sketch);
    assert_eq!(h.app.session.selection, selection);
    assert_eq!(h.app.session.doc.to_json(), before);
}

#[test]
fn typing_s_in_the_quick_menu_editor_filters_without_opening_a_menu() {
    let mut h = crate::scenario::Harness::new();
    h.app.home.open = false;
    customize(&mut h.app);
    h.frames(3);
    h.step(&json!({"click":{"handle":"quick_menu:filter"}})).unwrap();
    h.frames(2);
    h.step(&json!({"key":"S"})).unwrap();
    h.step(&json!({"text":"s"})).unwrap();
    h.frames(2);
    assert_eq!(h.app.quick_menu.filter, "s");
    assert!(h.app.quick_menu.editor);
    assert!(h.app.viewport.context_menu.is_none());
    assert!(!h.app.ui.palette_open);
}

#[test]
fn escape_closes_customization_without_cancelling_the_underlying_sketch_tool() {
    let mut h = sketch();
    h.app.start("sketch.line");
    customize(&mut h.app);
    h.frames(3);
    h.step(&json!({"click":{"handle":"quick_menu:filter"}})).unwrap();
    h.step(&json!({"text":"measure"})).unwrap();
    h.frames(2);
    h.step(&json!({"key":"Escape"})).unwrap();
    h.frames(2);
    assert!(!h.app.quick_menu.editor);
    assert_eq!(h.app.tool.as_ref().map(|tool| tool.cmd), Some("sketch.line"));
    assert!(h.app.session.active_sketch.is_some());
    h.step(&json!({"move":[600,400]})).unwrap();
    h.step(&json!({"key":"S"})).unwrap();
    h.frames(2);
    assert!(h.app.viewport.context_menu.is_some(), "S works again after dismissing customization");
}

#[test]
fn escape_closes_customization_without_cancelling_a_feature_dialog() {
    for modern in [false, true] {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        h.app.preferences.viewport_status_details = modern;
        h.app.start("solid.extrude");
        h.frames(3);
        assert!(h.app.dialog.is_some());
        customize(&mut h.app);
        h.frames(3);
        h.step(&json!({"key":"Escape"})).unwrap();
        h.frames(2);
        assert!(!h.app.quick_menu.editor);
        assert!(h.app.dialog.is_some(), "customization owns Escape with viewport status {modern}");
    }
}

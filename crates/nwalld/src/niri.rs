use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[derive(Debug, Default, Clone)]
pub struct NiriState {
    pub overview_open: bool,
    pub dragging_outputs: Vec<String>,
    pub fullscreen_outputs: Vec<String>,
    pub covered_outputs: Vec<String>,
    win_to_output: HashMap<u64, String>,
    focused_output: Option<String>,
}

pub struct NiriWatcher {
    pub state: Arc<std::sync::Mutex<NiriState>>,
    dirty: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl NiriWatcher {
    pub fn start() -> Result<Self> {
        let state = Arc::new(std::sync::Mutex::new(NiriState::default()));
        let dirty = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));

        {
            let mut s = state.lock().unwrap();
            refresh_snapshot(&mut s)?;
        }

        let state_c = Arc::clone(&state);
        let dirty_c = Arc::clone(&dirty);
        let stop_c = Arc::clone(&stop);

        let join = thread::Builder::new()
            .name("nwall-niri".into())
            .spawn(move || {
                if let Err(e) = event_loop(state_c, dirty_c, stop_c) {
                    log::warn!("niri watcher stopped: {e:#}");
                }
            })?;

        Ok(Self {
            state,
            dirty,
            stop,
            join: Some(join),
        })
    }

    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }

    pub fn snapshot(&self) -> NiriState {
        self.state.lock().unwrap().clone()
    }
}

impl Drop for NiriWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn event_loop(
    state: Arc<std::sync::Mutex<NiriState>>,
    dirty: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let mut child = Command::new("niri")
        .args(["msg", "-j", "event-stream"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn niri msg event-stream")?;
    let stdout = child.stdout.take().context("stdout")?;
    let reader = BufReader::new(stdout);

    for line in reader.lines() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let mut changed = false;
        {
            let mut s = state.lock().unwrap();
            if let Some(obj) = v.as_object() {
                if let Some(ov) = obj.get("OverviewOpenedOrClosed") {
                    if let Some(open) = ov.get("is_open").and_then(|x| x.as_bool()) {
                        if s.overview_open != open {
                            s.overview_open = open;
                            changed = true;
                        }
                    }
                }
                if obj.contains_key("WindowsChanged")
                    || obj.contains_key("WindowOpenedOrChanged")
                    || obj.contains_key("WindowClosed")
                    || obj.contains_key("WindowFocusChanged")
                    || obj.contains_key("WindowLayoutsChanged")
                    || obj.contains_key("WorkspacesChanged")
                    || obj.contains_key("WorkspaceActivated")
                    || obj.contains_key("OutputsChanged")
                {
                    if refresh_snapshot(&mut s).is_ok() {
                        changed = true;
                    }
                }
                if let Some(wlc) = obj.get("WindowLayoutsChanged") {
                    let outs = dragging_outputs_from_layouts(wlc, &s);
                    if s.dragging_outputs != outs {
                        s.dragging_outputs = outs;
                        changed = true;
                    } else if !s.dragging_outputs.is_empty() {
                        changed = true;
                    }
                } else if obj.contains_key("WindowFocusChanged")
                    || obj.contains_key("WindowsChanged")
                {
                    if !s.dragging_outputs.is_empty() {
                        s.dragging_outputs.clear();
                        changed = true;
                    }
                }
            }
        }
        if changed {
            dirty.store(true, Ordering::SeqCst);
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

fn dragging_outputs_from_layouts(wlc: &Value, s: &NiriState) -> Vec<String> {
    let mut outs: HashSet<String> = HashSet::new();
    if let Some(changes) = wlc.get("changes").and_then(|c| c.as_array()) {
        for ch in changes {
            let wid = ch
                .as_array()
                .and_then(|a| a.first())
                .and_then(|x| x.as_u64());
            if let Some(wid) = wid {
                if let Some(out) = s.win_to_output.get(&wid) {
                    outs.insert(out.clone());
                }
            }
        }
    }
    if outs.is_empty() {
        if let Some(fo) = &s.focused_output {
            outs.insert(fo.clone());
        }
    }
    let mut v: Vec<String> = outs.into_iter().collect();
    v.sort();
    v
}

fn refresh_snapshot(state: &mut NiriState) -> Result<()> {
    if let Ok(out) = Command::new("niri")
        .args(["msg", "-j", "overview-state"])
        .output()
    {
        if out.status.success() {
            if let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) {
                state.overview_open = v
                    .get("is_open")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
            }
        }
    }

    let outs: Value = json_msg(&["msg", "-j", "outputs"])?;
    let wins: Value = json_msg(&["msg", "-j", "windows"])?;
    let spaces: Value = json_msg(&["msg", "-j", "workspaces"])?;

    let mut sizes: HashMap<String, (f64, f64)> = HashMap::new();
    if let Some(map) = outs.as_object() {
        for (name, o) in map {
            if let Some(logical) = o.get("logical") {
                let w = logical.get("width").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let h = logical
                    .get("height")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0);
                if w > 0.0 && h > 0.0 {
                    sizes.insert(name.clone(), (w, h));
                }
            }
        }
    }

    let mut ws_out: HashMap<u64, String> = HashMap::new();
    let mut active_ws: HashSet<u64> = HashSet::new();
    let mut focused_output: Option<String> = None;
    let ws_list = if let Some(arr) = spaces.as_array() {
        arr.clone()
    } else if let Some(obj) = spaces.as_object() {
        if let Some(arr) = obj.get("workspaces").and_then(|x| x.as_array()) {
            arr.clone()
        } else {
            obj.values().cloned().collect()
        }
    } else {
        Vec::new()
    };
    for ws in ws_list {
        let id = ws.get("id").and_then(|x| x.as_u64());
        let output = ws
            .get("output")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        if let (Some(id), Some(output)) = (id, output) {
            if ws.get("is_focused").and_then(|x| x.as_bool()) == Some(true) {
                focused_output = Some(output.clone());
            }
            ws_out.insert(id, output);
            if ws.get("is_active").and_then(|x| x.as_bool()) == Some(true) {
                active_ws.insert(id);
            }
        }
    }
    state.focused_output = focused_output;

    let win_list = wins.as_array().cloned().unwrap_or_default();
    let mut fullscreen = Vec::new();
    let mut covered: HashMap<String, f64> = HashMap::new();
    let mut win_to_output: HashMap<u64, String> = HashMap::new();

    for win in &win_list {
        let ws_id = win.get("workspace_id").and_then(|x| x.as_u64());
        let Some(ws_id) = ws_id else { continue };
        let Some(out_name) = ws_out.get(&ws_id).cloned() else {
            continue;
        };
        if let Some(wid) = win.get("id").and_then(|x| x.as_u64()) {
            win_to_output.insert(wid, out_name.clone());
        }
        if !active_ws.contains(&ws_id) {
            continue;
        }
        let Some(&(ow, oh)) = sizes.get(&out_name) else {
            continue;
        };
        let lay = win.get("layout").cloned().unwrap_or(Value::Null);
        let off = lay
            .get("window_offset_in_tile")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let ts = lay
            .get("tile_size")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        let wsz = lay
            .get("window_size")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();

        let ox = off.first().and_then(|x| x.as_f64()).unwrap_or(1.0);
        let oy = off.get(1).and_then(|x| x.as_f64()).unwrap_or(1.0);
        let tw = ts.first().and_then(|x| x.as_f64()).unwrap_or(0.0);
        let th = ts.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let ww = wsz.first().and_then(|x| x.as_f64()).unwrap_or(0.0);
        let wh = wsz.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0);

        if ox == 0.0
            && oy == 0.0
            && (tw - ww).abs() <= 1.0
            && (th - wh).abs() <= 1.0
            && (tw - ow).abs() <= 1.0
            && (th - oh).abs() <= 1.0
        {
            if !fullscreen.contains(&out_name) {
                fullscreen.push(out_name.clone());
            }
        }

        let area = tw * th;
        let out_area = ow * oh;
        if out_area > 0.0 {
            let frac = area / out_area;
            let e = covered.entry(out_name).or_insert(0.0);
            *e = (*e).max(frac);
        }
    }

    state.win_to_output = win_to_output;
    state.fullscreen_outputs = fullscreen;
    state.covered_outputs = covered
        .into_iter()
        .filter(|(_, f)| *f >= 0.92)
        .map(|(n, _)| n)
        .collect();

    Ok(())
}

fn json_msg(args: &[&str]) -> Result<Value> {
    let out = Command::new("niri")
        .args(args)
        .output()
        .with_context(|| format!("niri {}", args.join(" ")))?;
    if !out.status.success() {
        return Err(anyhow::anyhow!(
            "niri {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

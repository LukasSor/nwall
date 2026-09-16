use std::path::Path;

use anyhow::{anyhow, Result};
use gtk::{DropDown, SpinButton, Switch};
use nwall_ipc::{client_request, FitMode, Request, Response};

pub(crate) fn ipc_ok(resp: Response) -> Result<()> {
    match resp {
        Response::Error { error } => Err(anyhow!("{error}")),
        Response::Ok { .. } | Response::Status(_) => Ok(()),
    }
}

pub(crate) fn apply_wallpaper(path: &Path, outputs: &[String]) -> Result<()> {
    ipc_ok(client_request(&Request::Set {
        path: path.to_path_buf(),
        output: None,
        outputs: outputs.to_vec(),
    })?)
}

pub(crate) fn push_playback(
    fps: &SpinButton,
    mute: &Switch,
    volume: &SpinButton,
    fit: &DropDown,
    video: bool,
) -> Result<()> {
    let mode = match fit.selected() {
        1 => FitMode::Contain,
        2 => FitMode::Stretch,
        _ => FitMode::Cover,
    };
    ipc_ok(client_request(&Request::SetFit { fit: mode })?)?;
    if !video {
        return Ok(());
    }
    ipc_ok(client_request(&Request::SetMute {
        mute: mute.is_active(),
    })?)?;
    ipc_ok(client_request(&Request::SetVolume {
        volume: (volume.value() as f32) / 100.0,
    })?)?;
    ipc_ok(client_request(&Request::SetFps {
        fps: fps.value() as u32,
    })?)?;
    Ok(())
}

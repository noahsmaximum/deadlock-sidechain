//! `sidechain selftest` — proves the two OS mechanisms on this machine without touching the
//! music app, using throwaway child processes:
//!   1. process loopback hears a process that plays on the (non-default) sink device;
//!   2. the per-app override reaches a running process as OnDefaultDeviceChanged(sink).
//! (Also established while building this: loopback taps *after* session mute/volume, which is why
//! the app is re-pointed to a sink instead of muted.)

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use wasapi::{AudioClient, DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

use crate::app::find_render;
use crate::audio::RATE;
use crate::routing::PolicyConfig;

fn fmt() -> WaveFormat {
    WaveFormat::new(32, 32, &SampleType::Float, RATE, 2, None)
}

/// `__tone <device|@default> <secs> <amp>` — play a 440 Hz sine; with @default, also print
/// default-device notifications so the parent can see them.
pub fn tone(device: &str, secs: f64, amp: f64) -> Result<()> {
    wasapi::initialize_mta().ok()?;
    let en = DeviceEnumerator::new()?;
    let mut cb = wasapi::DeviceEventCallbacks::new();
    cb.set_default_device_callback(|_, role, id| println!("default-changed {role:?} {}", id.unwrap_or_default()));
    let _reg = en.register_notification_callback(cb)?;
    let dev = if device == "@default" { en.get_default_device(&Direction::Render)? } else { find_render(device)? };
    let mut client = dev.get_iaudioclient()?;
    let (def, _) = client.get_device_period()?;
    client.initialize_client(&fmt(), &Direction::Render, &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: def })?;
    let ev = client.set_get_eventhandle()?;
    let rc = client.get_audiorenderclient()?;
    let mut ph = 0f64;
    let mut write = |n: usize| -> Result<()> {
        let mut buf = Vec::with_capacity(n * 8);
        for _ in 0..n {
            let s = (ph.sin() * amp) as f32;
            ph += 2.0 * std::f64::consts::PI * 440.0 / RATE as f64;
            buf.extend_from_slice(&s.to_le_bytes());
            buf.extend_from_slice(&s.to_le_bytes());
        }
        rc.write_to_device(n, &buf, None)?;
        Ok(())
    };
    write(client.get_available_space_in_frames()? as usize)?;
    client.start_stream()?;
    println!("playing");
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        ev.wait_for_event(1000)?;
        write(client.get_available_space_in_frames()? as usize)?;
    }
    Ok(())
}

fn loopback_rms(pid: u32, secs: f64) -> Result<f32> {
    let mut client = AudioClient::new_application_loopback_client(pid, true)?;
    client.initialize_client(&fmt(), &Direction::Capture, &StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 })?;
    let ev = client.set_get_eventhandle()?;
    let cc = client.get_audiocaptureclient()?;
    let mut q = VecDeque::new();
    client.start_stream()?;
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        if ev.wait_for_event(200).is_ok() {
            while cc.get_next_packet_size()?.unwrap_or(0) > 0 {
                cc.read_from_device_to_deque(&mut q)?;
            }
        }
    }
    client.stop_stream()?;
    let (a, b) = q.as_slices();
    let bytes: Vec<u8> = a.iter().chain(b).copied().collect();
    let s: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    Ok((s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt())
}

pub fn selftest(sink: &str) -> Result<()> {
    wasapi::initialize_mta().ok()?;
    let exe = std::env::current_exe()?;
    let sink_dev = find_render(sink)?;
    let sink_id = sink_dev.get_id()?;
    println!("sink: {} {}", sink_dev.get_friendlyname()?, sink_id);

    // 1. capture from the sink (tone at -17 dBFS on the sink — inaudible)
    let mut child = Command::new(&exe).args(["__tone", sink, "4", "0.2"]).stdout(Stdio::null()).spawn()?;
    std::thread::sleep(Duration::from_millis(700));
    let rms = loopback_rms(child.id(), 1.5);
    let _ = child.kill();
    let rms = rms?;
    let pass1 = rms > 0.05;
    println!("[{}] process loopback hears a process playing on the sink (RMS {rms:.3}, expect ~0.141)", if pass1 { "PASS" } else { "FAIL" });

    // 2. per-app override reaches a running process (silent tone on the default device)
    let pc = PolicyConfig::new()?;
    let mut child = Command::new(&exe).args(["__tone", "@default", "5", "0"]).stdout(Stdio::piped()).spawn()?;
    let mut lines = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?).lines();
    let _ = lines.next(); // "playing"
    std::thread::sleep(Duration::from_millis(300));
    let set = pc.set_render_device(child.id(), Some(&sink_id));
    let readback = pc.render_device(child.id());
    let mut seen = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for l in lines.map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    while Instant::now() < deadline && !seen {
        if let Ok(l) = rx.recv_timeout(Duration::from_millis(200)) {
            seen |= l.starts_with("default-changed") && l.contains(&sink_id);
        }
    }
    let _ = pc.set_render_device(child.id(), None);
    let _ = child.kill();
    let pass2 = set.is_ok() && seen;
    println!(
        "[{}] per-app override: set {} / readback {} / process notified of sink {}",
        if pass2 { "PASS" } else { "FAIL" },
        if set.is_ok() { "ok" } else { "FAILED" },
        if matches!(readback, Ok(Some(_))) { "ok" } else { "missing" },
        if seen { "yes" } else { "no" }
    );
    if pass1 && pass2 {
        println!("all good — the app will move to the sink and be captured from there.");
    }
    Ok(())
}

/// `sidechain devices`
pub fn devices(sink: &str) -> Result<()> {
    wasapi::initialize_mta().ok()?;
    let en = DeviceEnumerator::new()?;
    let def = en.get_default_device(&Direction::Render)?.get_id()?;
    let coll = en.get_device_collection(&Direction::Render)?;
    for i in 0..coll.get_nbr_devices()? {
        let d = coll.get_device_at_index(i)?;
        let name = d.get_friendlyname()?;
        let id = d.get_id()?;
        let mut tags = Vec::new();
        if id == def {
            tags.push("default");
        }
        if !sink.is_empty() && name.to_lowercase().contains(&sink.to_lowercase()) {
            tags.push("sink");
        }
        println!("{:<48} {}", name, if tags.is_empty() { String::new() } else { format!("[{}]", tags.join(", ")) });
    }
    Ok(())
}

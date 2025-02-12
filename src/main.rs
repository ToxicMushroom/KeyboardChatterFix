#![feature(if_let_guard)]
#![feature(let_chains)]

use std::cmp::Ordering;
use std::fs;
use std::fs::File;
use std::io::{Error, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use evdev::uinput::VirtualDevice;
use evdev::{enumerate, Device, EnumerateDevices, EventSummary, EventType, InputEvent, KeyCode};
use ini::ini;
use tokio::select;

#[derive(PartialEq, Eq)]
struct KeyPress {
    key: KeyCode,
    time: SystemTime,
}

impl PartialOrd<Self> for KeyPress {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KeyPress {
    fn cmp(&self, other: &Self) -> Ordering {
        self.time.cmp(&other.time)
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut devices = enumerate();
    let (threshold_dur, keyboard_id) = load_config()?;
    let (event_device_path, mut device) = find_keyboard(&mut devices, keyboard_id);

    println!("Hooked {:?} at {:?}", device.name(), event_device_path);
    let keys = device.supported_keys().unwrap();
    let mut fake_keyboard = VirtualDevice::builder()?
        .name("Chatter Fix Emulated Keyboard")
        .with_keys(keys)?
        .build()?;

    for path in fake_keyboard.enumerate_dev_nodes_blocking()? {
        let path = path?;
        println!("Available as {}", path.display());
    }

    let mut pressed_hist = [SystemTime::UNIX_EPOCH; 0x2e7];
    let mut backlog: Vec<KeyPress> = vec![];

    device.grab()
        .expect("Could not grab (take full control of) your device");
    println!("Started main event loop.");
    let mut event_stream = device.into_event_stream()?;
    loop {
        let current_time = SystemTime::now();

        let ev = if backlog.is_empty() {
            Some(event_stream.next_event().await?)
        } else {
            let wait_for = (backlog[0].time + threshold_dur) // the time in the future
                .duration_since(current_time) // duration until that time
                .unwrap_or(Duration::ZERO); // or 0, if the 'future' is in the past

            select! {
                ev = event_stream.next_event() => Some(ev?),
                _ = tokio::time::sleep(wait_for) => None,
            }
        };

        match ev {
            Some(ev) if let EventSummary::Key(_key_ev, key, _value) = ev.destructure() => {
                let key_press = KeyPress {
                    key,
                    time: ev.timestamp(),
                };
                let pressed = ev.value() == 1;
                let idx = key.code() as usize;
                let hist = pressed_hist[idx];
                if pressed {
                    // Add key to press history
                    pressed_hist[idx] = ev.timestamp();

                    // Remove key from backlog
                    let pos = backlog
                        .iter()
                        .position(|key_press: &KeyPress| key_press.key == key);
                    if let Some(pos) = pos {
                        backlog.remove(pos);
                        println!("Chatter prevented.");
                    }
                } else {
                    // If depressed within threshold
                    if let Ok(diff) = SystemTime::now().duration_since(hist)
                        && diff < threshold_dur
                    {
                        // filter the release keypress and add to backlog
                        backlog.push(key_press);
                        continue;
                    }
                }

                // Events that occurred normally (outside threshold) are just emitted
                if fake_keyboard.emit(&[ev]).is_err() {
                    eprintln!("Could not emit keypress");
                }
            }
            None => {
                // First item that has been backlogged for at least threshold duration
                let old_backlog_item = &backlog[0];

                // Emit found item
                if fake_keyboard
                    .emit(&[InputEvent::new(
                        EventType::KEY.0,
                        old_backlog_item.key.code(),
                        0,
                    )])
                    .is_err()
                {
                    eprintln!("Could not emit key release");
                }
                backlog.remove(0);
            }
            _ => {}
        };
    }
}

fn find_keyboard(devices: &mut EnumerateDevices, kid: String) -> (PathBuf, Device) {
    let link = fs::read_link(format!("/dev/input/by-id/{kid}"));

    let (path, dev) = if let Ok(link) = link {
        let eventn = link.as_path().file_name().unwrap(); // "../event9" -> "event9"
        devices
            .find(|(path, _d)| {
                path.as_path().file_name().unwrap() == eventn
            })
            .expect("Found no matching keyboard")
    } else {
        devices
            .find(|(_p, dev)| {
                dev.name().unwrap_or("").contains(kid.as_str())
                    && dev
                    .supported_keys()
                    .is_some_and(|keys| keys.contains(KeyCode::KEY_ENTER))
            })
            .expect("Found no matching keyboard")
    };
    (path, dev)
}

fn load_config() -> Result<(Duration, String), Error> {
    let config_path = option_env!("XDG_CONFIG_PATH").map_or_else(
        || {
            option_env!("HOME")
                .map(|s| format!("{s}/.config"))
                .expect("You don't have $HOME set, I can't look for a .config folder ?")
        },
        String::from,
    );
    let our_config_path = format!("{config_path}/keyboard-chatter-fix/config.ini");
    let config_dir = Path::new(&our_config_path);
    if !config_dir.exists() {
        let mut file = File::create(&our_config_path)?;
        file.write_all(b"id = Ducky One 3\nthreshold = 30")?;
    }

    let ini = ini!(our_config_path.as_str());
    let binding = ini["default"]["id"].clone().unwrap_or(String::new());

    let threshold = ini["default"]["threshold"]
        .clone()
        .map_or(30, |string: String| string.parse::<u32>().unwrap());

    let threshold_dur = Duration::from_millis(threshold as u64);

    Ok((threshold_dur, binding))
}

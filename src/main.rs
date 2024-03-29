#![feature(if_let_guard)]
#![feature(let_chains)]

use std::cmp::Ordering;
use std::fs::File;
use std::io::{Error, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use evdev::{enumerate, EventType, InputEvent, InputEventKind, Key};
use evdev::uinput::VirtualDeviceBuilder;
use ini::ini;
use tokio::select;

#[derive(PartialEq, Eq)]
struct KeyPress {
    key: Key,
    time: SystemTime,
}

impl PartialOrd<Self> for KeyPress {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.time.partial_cmp(&other.time)
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
    let config_path = option_env!("XDG_CONFIG_PATH")
        .map_or_else(||
                         option_env!("HOME")
                             .map(|s| format!("{s}/.config"))
                             .expect("You don't have $HOME set, I can't look for a .config folder ?"),
                     |t| String::from(t),
        );
    let our_config_path = format!("{config_path}/keyboard-chatter-fix/config.ini");
    let config_dir = Path::new(&our_config_path);
    if !config_dir.exists() {
        let mut file = File::create(&our_config_path)?;
        file.write_all(b"id = Ducky One 3\nthreshold = 30")?;
    }
    let ini = ini!(our_config_path.as_str());
    let binding = ini["default"]["id"].clone().unwrap_or(String::new());
    let kid = binding.as_str();
    let threshold = ini["default"]["threshold"].clone().map_or(30, |string: String| { string.parse::<u32>().unwrap() });
    let threshold_dur = Duration::from_millis(threshold as u64);

    let (_, mut dev) = devices
        .find(|(_path, dev)| {
            dev.name().unwrap_or("").contains(kid) &&
                dev.supported_keys().map_or(false, |keys| { keys.contains(Key::KEY_ENTER) })
        })
        .expect("Found no matching keyboard");

    let keys = dev.supported_keys().unwrap();
    let mut fake_keyboard = VirtualDeviceBuilder::new()?
        .name("Chatter Fix Emulated Keyboard")
        .with_keys(&keys)?
        .build()
        .unwrap();

    for path in fake_keyboard.enumerate_dev_nodes_blocking()? {
        let path = path?;
        println!("Available as {}", path.display());
    }

    let mut pressed_hist = [SystemTime::UNIX_EPOCH; 0x2e7];
    let mut backlog: Vec<KeyPress> = vec![];

    dev.grab().expect("Could not grab (take full control of) your device");
    println!("Started main event loop.");
    let mut event_stream = dev.into_event_stream()?;
    loop {
        let current_time = SystemTime::now();

        let ev = if backlog.is_empty() {
            Some(event_stream.next_event().await.unwrap())
        } else {
            let wait_for = (backlog[0].time + threshold_dur) // the time in the future
                .duration_since(current_time) // duration until that time
                .unwrap_or(Duration::ZERO); // or 0, if the 'future' is in the past

            select! {
                ev = event_stream.next_event() => Some(ev.unwrap()),
                _ = tokio::time::sleep(wait_for) => None,
            }
        };

        match ev {
            Some(ev) if let InputEventKind::Key(key) = ev.kind() => {
                let key_press = KeyPress { key, time: ev.timestamp() };
                let pressed = ev.value() == 1;
                let idx = key.code() as usize;
                let hist = pressed_hist[idx];
                if pressed {
                    // Add key to press history
                    pressed_hist[idx] = ev.timestamp();

                    // Remove key from backlog
                    let pos = backlog.iter().position(|key_press: &KeyPress| { key_press.key == key });
                    if let Some(pos) = pos {
                        backlog.remove(pos);
                        println!("Chatter prevented.");
                    }
                } else {
                    // If depressed within threshold
                    if let Ok(diff) = SystemTime::now().duration_since(hist) && diff < threshold_dur {
                        // filter the release keypress and add to backlog
                        backlog.push(key_press);
                        continue;
                    }
                }

                // Events that occurred normally (outside threshold) are just emitted
                if fake_keyboard.emit(&[ev]).is_err() {
                    eprintln!("Could not emit keypress");
                }
            },
            None => {
                // First item that has been backlogged for at least threshold duration
                let old_backlog_item = &backlog[0];

                // Emit found item
                if fake_keyboard.emit(&[InputEvent::new(EventType::KEY, old_backlog_item.key.code(), 0)]).is_err() {
                    eprintln!("Could not emit key release");
                }
                backlog.remove(0);
            },
            _ => {}
        };
    }
}

use std::any::{Any, TypeId};
use std::array::from_fn;
use std::cmp::Ordering;
use std::fmt::format;
use std::fs::File;
use std::io::{Error, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};
use evdev::{AttributeSet, enumerate, EnumerateDevices, EventType, InputEvent, InputEventKind, Key, MiscType, PropType};
use evdev::uinput::VirtualDeviceBuilder;
use ini::ini;
use ringbuffer::{ConstGenericRingBuffer, RingBuffer};

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

fn main() -> Result<(), Error> {
    use evdev::{Device, Key};
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
    let threshold = ini["default"]["threshold"].clone().map_or_else(|| { 30 }, |string: String| { string.parse::<u32>().unwrap() });

    let (_, mut dev) = devices
        .find(|(path, dev)| {
            dev.name().unwrap_or("").contains(kid) &&
                dev.supported_keys().map_or(false, |keys| { keys.contains(Key::KEY_ENTER) })
        })
        .expect("Found no matching keyboard");

    let keys = dev.supported_keys().unwrap();
    let mut fake_keyboard = VirtualDeviceBuilder::new()?
        .name("Fake Keyboard")
        .with_keys(&keys)?
        .build()
        .unwrap();

    for path in fake_keyboard.enumerate_dev_nodes_blocking()? {
        let path = path?;
        println!("Available as {}", path.display());
    }

    let mut pressed_hist: Arc<Mutex<[KeyPress; 0x2e7]>> = Arc::new(Mutex::new(from_fn(|i| {
        KeyPress { key: Key(i as u16), time: SystemTime::UNIX_EPOCH }
    })));
    let mut backlog: Arc<Mutex<Vec<KeyPress>>> = Arc::new(Mutex::new(vec![]));

    let mut backlog2 = backlog.clone();
    let mut fake_keyboard = Arc::new(Mutex::new(fake_keyboard));
    let mut fake_keyboard2 = fake_keyboard.clone();

    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(1));

            // Find item that has been backlogged for at least 30ms
            let mut mutable_backlog = backlog2.lock().unwrap();
            let pos = mutable_backlog.iter().position(|el| { el.time < (SystemTime::now() - Duration::from_millis(threshold as u64)) });
            if let Some(pos) = pos {
                let old_backlog_item = &mutable_backlog[pos];


                // Emit found item
                let mut fake_keyboard = fake_keyboard2.lock().unwrap();
                fake_keyboard.emit(&[InputEvent::new(EventType::KEY, old_backlog_item.key.0, 0)]).expect("Could not emit key release");
                mutable_backlog.remove(pos);
            }
        }
    });

    loop {
        dev.grab().expect("Could not grab (take full control of) your device");
        for ev in dev.fetch_events().unwrap() {
            match ev.kind() {
                InputEventKind::Key(key) => {
                    let key_press = KeyPress { key, time: ev.timestamp() };
                    let pressed = ev.value() == 1;
                    let idx = key.clone().0 as usize;

                    let mut pressed_hist = pressed_hist.lock().unwrap();
                    let hist = &pressed_hist[idx];
                    if pressed {
                        // Add key to press history
                        pressed_hist[idx] = key_press;
                        let mut mutable_backlog = backlog.lock().unwrap();

                        // Remove key from backlog
                        let pos = mutable_backlog.iter().position(|key_press: &KeyPress| { key_press.key == key });
                        if let Some(pos) = pos {
                            mutable_backlog.remove(pos);
                        }

                    } else {
                        let time_diff = SystemTime::now().min(hist.time);
                        let time_diff = time_diff.duration_since(SystemTime::UNIX_EPOCH).expect("Can't convert time_diff to a duration ?");
                        if time_diff < Duration::from_millis(threshold as u64) {
                            // If depressed within threshold, filter the release keypress and add to backlog
                            let mut mutable_backlog = backlog.lock().unwrap();
                            mutable_backlog.push(key_press);
                            continue;
                        }
                    }

                    // Events that occurred normally (outside threshold) are just emitted
                    let mut fake_keyboard = fake_keyboard.lock().unwrap();
                    fake_keyboard.emit(&[ev]).expect("Could not emit keypress");
                    println!("{ev:?}");
                }
                _ => {}
            }
        }
    }
}

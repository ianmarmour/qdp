//! What the user chose for every stream: the video's size and decoding,
//! sound, microphone, clipboard and touchpad, kept in a file of the user's
//! configuration, a `key=value` line each, and passed to `qdp connect` as
//! its options.

use std::fs;
use std::io;
use std::path::Path;

/// The fixed sizes the video can be asked for, besides the window's.
pub const FIXED_RESOLUTIONS: [(u32, u32); 4] =
    [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];

/// The size of the video asked of the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resolution {
    /// The window's size, up to the host's screen's.
    #[default]
    Window,
    /// A fixed size, which the window stretches: less to stream at a large
    /// or full-screen window.
    Fixed {
        /// The width, in pixels.
        width: u32,
        /// The height, in pixels.
        height: u32,
    },
}

impl Resolution {
    /// Every choice, in the order the settings list them.
    pub fn choices() -> Vec<Self> {
        std::iter::once(Self::Window)
            .chain(
                FIXED_RESOLUTIONS
                    .iter()
                    .map(|&(width, height)| Self::Fixed { width, height }),
            )
            .collect()
    }

    /// Returns the choice's name for users.
    pub fn label(self) -> String {
        match self {
            Self::Window => "Window size".to_owned(),
            Self::Fixed { width, height } => format!("{width} × {height}"),
        }
    }
}

/// How the video is decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Decoder {
    /// By the GPU when it can, otherwise in software.
    #[default]
    Automatic,
    /// By the GPU.
    Hardware,
    /// In software.
    Software,
}

impl Decoder {
    /// Every choice, in the order the settings list them.
    pub const CHOICES: [Self; 3] = [Self::Automatic, Self::Hardware, Self::Software];

    /// Returns the choice's name for users.
    pub fn label(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Hardware => "Hardware",
            Self::Software => "Software",
        }
    }

    /// Returns the choice as `qdp connect --decoder` takes it.
    fn option(self) -> &'static str {
        match self {
            Self::Automatic => "auto",
            Self::Hardware => "hardware",
            Self::Software => "software",
        }
    }
}

/// Whether the host's sound plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sound {
    /// It plays.
    #[default]
    On,
    /// It does not.
    Off,
}

/// Whether this computer's microphone is sent to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Microphone {
    /// It is not.
    #[default]
    Off,
    /// It is.
    On,
}

/// Which way the clipboard's text is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Clipboard {
    /// Both ways.
    #[default]
    Both,
    /// Only this computer's, to the host.
    Send,
    /// Only the host's, to this computer.
    Receive,
    /// Not at all.
    Off,
}

impl Clipboard {
    /// Every choice, in the order the settings list them.
    pub const CHOICES: [Self; 4] = [Self::Both, Self::Send, Self::Receive, Self::Off];

    /// Returns the choice's name for users.
    pub fn label(self) -> &'static str {
        match self {
            Self::Both => "Both ways",
            Self::Send => "Only this computer's",
            Self::Receive => "Only the host's",
            Self::Off => "Off",
        }
    }

    /// Returns the choice as `qdp connect --clipboard` takes it.
    fn option(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Send => "send",
            Self::Receive => "receive",
            Self::Off => "off",
        }
    }
}

/// What the touchpad does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Touchpad {
    /// It moves the pointer, as a mouse.
    #[default]
    Pointer,
    /// Its fingers go to the host, for the host's own gestures.
    Gestures,
}

/// The user's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settings {
    /// The video's size.
    pub resolution: Resolution,
    /// How the video is decoded.
    pub decoder: Decoder,
    /// Whether the host's sound plays.
    pub sound: Sound,
    /// Whether the microphone is sent.
    pub microphone: Microphone,
    /// Which way the clipboard is shared.
    pub clipboard: Clipboard,
    /// What the touchpad does.
    pub touchpad: Touchpad,
}

impl Settings {
    /// Reads the settings in `path`: the defaults for any missing, and all
    /// of them if there is no file.
    pub fn load(path: &Path) -> Self {
        let mut settings = Self::default();
        let Ok(text) = fs::read_to_string(path) else {
            return settings;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            settings.read(key.trim(), value.trim());
        }
        settings
    }

    /// Reads one setting, leaving the default for a value it does not know.
    fn read(&mut self, key: &str, value: &str) {
        match (key, value) {
            ("resolution", "window") => self.resolution = Resolution::Window,
            ("resolution", size) => {
                if let Some((width, height)) = size.split_once('x')
                    && let (Ok(width), Ok(height)) = (width.parse(), height.parse())
                {
                    self.resolution = Resolution::Fixed { width, height };
                }
            }
            ("decoder", value) => {
                if let Some(decoder) = Decoder::CHOICES
                    .into_iter()
                    .find(|choice| choice.option() == value)
                {
                    self.decoder = decoder;
                }
            }
            ("sound", "on") => self.sound = Sound::On,
            ("sound", "off") => self.sound = Sound::Off,
            ("microphone", "on") => self.microphone = Microphone::On,
            ("microphone", "off") => self.microphone = Microphone::Off,
            ("clipboard", value) => {
                if let Some(clipboard) = Clipboard::CHOICES
                    .into_iter()
                    .find(|choice| choice.option() == value)
                {
                    self.clipboard = clipboard;
                }
            }
            ("touchpad", "pointer") => self.touchpad = Touchpad::Pointer,
            ("touchpad", "gestures") => self.touchpad = Touchpad::Gestures,
            _ => {}
        }
    }

    /// Writes the settings to `path`.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        let resolution = match self.resolution {
            Resolution::Window => "window".to_owned(),
            Resolution::Fixed { width, height } => format!("{width}x{height}"),
        };
        let on = |on: bool| if on { "on" } else { "off" };
        let text = format!(
            "resolution={resolution}\ndecoder={}\nsound={}\nmicrophone={}\nclipboard={}\ntouchpad={}\n",
            self.decoder.option(),
            on(self.sound == Sound::On),
            on(self.microphone == Microphone::On),
            self.clipboard.option(),
            match self.touchpad {
                Touchpad::Pointer => "pointer",
                Touchpad::Gestures => "gestures",
            },
        );
        fs::write(path, text)
    }

    /// Returns the options of `qdp connect` that carry the settings; none for
    /// those left as `qdp connect` has them by default.
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        if let Resolution::Fixed { width, height } = self.resolution {
            arguments.extend(["--resolution".to_owned(), format!("{width}x{height}")]);
        }
        if self.decoder != Decoder::Automatic {
            arguments.extend(["--decoder".to_owned(), self.decoder.option().to_owned()]);
        }
        if self.sound == Sound::Off {
            arguments.extend(["--audio".to_owned(), "off".to_owned()]);
        }
        if self.microphone == Microphone::On {
            arguments.push("--microphone".to_owned());
        }
        if self.clipboard != Clipboard::Both {
            arguments.extend(["--clipboard".to_owned(), self.clipboard.option().to_owned()]);
        }
        if self.touchpad == Touchpad::Gestures {
            arguments.push("--touchpad".to_owned());
        }
        arguments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_only_what_differs_from_qdps_defaults() {
        assert!(Settings::default().arguments().is_empty());
        let settings = Settings {
            resolution: Resolution::Fixed {
                width: 1920,
                height: 1080,
            },
            decoder: Decoder::Software,
            sound: Sound::Off,
            microphone: Microphone::On,
            clipboard: Clipboard::Receive,
            touchpad: Touchpad::Gestures,
        };
        assert_eq!(
            settings.arguments(),
            [
                "--resolution",
                "1920x1080",
                "--decoder",
                "software",
                "--audio",
                "off",
                "--microphone",
                "--clipboard",
                "receive",
                "--touchpad"
            ]
        );
    }

    #[test]
    fn keeps_settings_in_a_file() {
        let path = std::env::temp_dir()
            .join(format!("qdp-gtk-settings-{}", std::process::id()))
            .join("settings");
        assert_eq!(Settings::load(&path), Settings::default());
        let settings = Settings {
            resolution: Resolution::Fixed {
                width: 2560,
                height: 1440,
            },
            decoder: Decoder::Hardware,
            sound: Sound::Off,
            microphone: Microphone::On,
            clipboard: Clipboard::Off,
            touchpad: Touchpad::Gestures,
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
        // A line it does not know leaves the default.
        fs::write(&path, "decoder=quantum\nsound=off\nnot a setting\n").unwrap();
        let read = Settings::load(&path);
        assert_eq!(read.decoder, Decoder::Automatic);
        assert_eq!(read.sound, Sound::Off);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn lists_its_choices() {
        let resolutions = Resolution::choices();
        assert_eq!(resolutions[0], Resolution::Window);
        assert_eq!(resolutions[2].label(), "1920 × 1080");
        assert_eq!(Clipboard::CHOICES[1].label(), "Only this computer's");
    }
}

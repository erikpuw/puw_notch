//! Line icons of the status banners and Bluetooth devices, and how they move.
//!
//! Each icon is a few SVG paths on a square grid, stroked with round caps. The shapes come from
//! Tabler Icons (MIT) and, for Bluetooth, the plug, the plane and the charging battery, from the
//! animated set on itshover.com; the motion is written here. Parts that move on their own sit in
//! a group (0..3, each with an origin to turn and scale about); a group also carries an opacity.

use crate::render::*;

/// One path of an icon: `g` = the group that moves it, `a` = the group whose opacity fades it
/// (NONE = none).
pub struct Part {
    pub d: &'static str,
    pub g: u8,
    pub a: u8,
}

pub const NONE: u8 = 255;

const fn p(d: &'static str) -> Part {
    Part { d, g: NONE, a: NONE }
}
const fn moves(d: &'static str, g: u8) -> Part {
    Part { d, g, a: NONE }
}
const fn fades(d: &'static str, a: u8) -> Part {
    Part { d, g: NONE, a }
}

/// How an icon moves in the first moments of its banner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anim {
    Bluetooth,
    Sun,
    Charging,
    Unplug,
    Plane,
    Volume,
    Mute,
    Mic,
    MicOff,
    Keyboard,
    Wifi,
    WifiOff,
    Pulse,
    Ring,
    BatteryLow,
    Saver,
    Bell,
    Walk,
    Headphones,
    SpeakerCone,
    MouseWheel,
    Gamepad,
}

pub struct Icon {
    /// Side of the square grid the paths are on.
    pub vb: f32,
    /// Stroke width on that grid.
    pub sw: f32,
    pub parts: &'static [Part],
    /// Where each group turns and scales from.
    pub origins: [(f32, f32); 4],
    pub anim: Anim,
}

const MID: [(f32, f32); 4] = [(12.0, 12.0); 4];

static BLUETOOTH: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Bluetooth, origins: [(12.0, 12.0), (4.5, 12.0), (18.5, 12.0), (0.0, 0.0)],
    parts: &[moves("M7 8l10 8l-5 4l0 -16l5 4l-10 8", 0), moves("M4 12l1 0", 1), moves("M18 12l1 0", 2)],
};
static SUN: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Sun, origins: MID,
    parts: &[moves("M12 12m-4 0a4 4 0 1 0 8 0a4 4 0 1 0 -8 0", 0), moves("M3 12h1m8 -9v1m8 8h1m-9 8v1m-6.4 -15.4l.7 .7m12.1 -.7l-.7 .7m0 11.4l.7 .7m-12.1 -.7l-.7 .7", 1)],
};
/// The charging battery of itshover: its four bars fill one by one.
static CHARGING: Icon = Icon {
    vb: 48.0, sw: 3.6, anim: Anim::Charging, origins: MID,
    parts: &[
        fades("M10 31V17", 0), fades("M17 31V17", 1), fades("M24 31V17", 2), fades("M31 31V17", 3),
        p("M46 20V28"),
        p("M37 10H8C5.2386 10 3 12.2386 3 15V33C3 35.7614 5.2386 38 8 38H37C39.7614 38 42 35.7614 42 33V15C42 12.2386 39.7614 10 37 10Z"),
    ],
};
/// The plug of itshover, its two halves pulling apart.
static PLUG: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Unplug, origins: MID,
    parts: &[
        moves("M7 12l5 5l-1.5 1.5a3.536 3.536 0 1 1 -5 -5l1.5 -1.5z", 0),
        moves("M17 12l-5 -5l1.5 -1.5a3.536 3.536 0 1 1 5 5l-1.5 1.5z", 1),
        moves("M3 21l2.5 -2.5", 0),
        moves("M18.5 5.5l2.5 -2.5", 1),
        Part { d: "M10 11l-2 2", g: 0, a: 2 },
        Part { d: "M13 14l-2 2", g: 0, a: 2 },
    ],
};
static PLANE: Icon = Icon {
    vb: 24.0, sw: 1.6, anim: Anim::Plane, origins: MID,
    parts: &[
        moves("M13.617 1.53c.926-.747 1.964-.793 3.097-.777.587.009.88.013 1.115.104.373.145.67.442.815.815.09.235.095.528.104 1.115.016 1.133-.03 2.17-.778 3.097-.63.78-1.714 1.188-2.235 2.05-.398.656-.213 1.337-.042 2.038l1.28 5.245c.255 1.046.06 1.715-.67 2.446-.39.39-.713.37-1.028-.133l-3.862-6.154-1.845 1.466c-.67.532-1.003.798-1.18 1.172-.41.876-.15 2.192-.136 3.142.008.525-.446 1.56-1.088 1.594-.396.022-.531-.452-.66-.743l-1.232-2.802c-.294-.67-.306-.682-.976-.976l-2.802-1.233c-.29-.128-.765-.263-.743-.66.035-.641 1.07-1.095 1.594-1.087.95.013 2.266.274 3.142-.137.374-.176.64-.51 1.172-1.18l1.466-1.844L1.97 4.226c-.503-.316-.522-.64-.133-1.028.73-.73 1.4-.925 2.446-.67l5.245 1.28c.7.17 1.382.356 2.039-.042.86-.52 1.269-1.604 2.049-2.235", 0),
        moves("m.5 3.5 3-3", 1),
        moves("m.5 3.5 3-3", 2),
    ],
};
static VOLUME: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Volume, origins: MID,
    parts: &[
        p("M6 15h-2a1 1 0 0 1 -1 -1v-4a1 1 0 0 1 1 -1h2l3.5 -4.5a.8 .8 0 0 1 1.5 .5v14a.8 .8 0 0 1 -1.5 .5l-3.5 -4.5"),
        fades("M15 8a5 5 0 0 1 0 8", 0),
        fades("M17.7 5a9 9 0 0 1 0 14", 1),
    ],
};
static MUTE: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Mute, origins: [(18.0, 12.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
    parts: &[p("M6 15h-2a1 1 0 0 1 -1 -1v-4a1 1 0 0 1 1 -1h2l3.5 -4.5a.8 .8 0 0 1 1.5 .5v14a.8 .8 0 0 1 -1.5 .5l-3.5 -4.5"), moves("M16 10l4 4m0 -4l-4 4", 0)],
};
static MIC: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Mic, origins: [(12.0, 7.5), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
    parts: &[moves("M9 2m0 3a3 3 0 0 1 3 -3h0a3 3 0 0 1 3 3v5a3 3 0 0 1 -3 3h0a3 3 0 0 1 -3 -3z", 0), p("M5 10a7 7 0 0 0 14 0"), p("M8 21l8 0"), p("M12 17l0 4")],
};
static MIC_OFF: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::MicOff, origins: MID,
    parts: &[
        moves("M3 3l18 18", 0),
        moves("M9 5a3 3 0 0 1 6 0v5a3 3 0 0 1 -.13 .874m-2 2a3 3 0 0 1 -3.87 -2.872v-1", 1),
        moves("M5 10a7 7 0 0 0 10.846 5.85m2 -2a6.967 6.967 0 0 0 1.152 -3.85", 1),
        moves("M8 21l8 0", 1),
        moves("M12 17l0 4", 1),
    ],
};
static KEYBOARD: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Keyboard, origins: MID,
    parts: &[
        p("M2 6m0 2a2 2 0 0 1 2 -2h16a2 2 0 0 1 2 2v8a2 2 0 0 1 -2 2h-16a2 2 0 0 1 -2 -2z"),
        moves("M6 10l0 .01", 0), moves("M10 10l0 .01", 0), moves("M14 10l0 .01", 0), moves("M18 10l0 .01", 0),
        moves("M6 14l0 .01", 0), moves("M18 14l0 .01", 0), moves("M10 14l4 .01", 0),
    ],
};
static WIFI: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Wifi, origins: MID,
    parts: &[p("M12 18l.01 0"), fades("M9.172 15.172a4 4 0 0 1 5.656 0", 0), fades("M6.343 12.343a8 8 0 0 1 11.314 0", 1), fades("M3.515 9.515c4.686 -4.687 12.284 -4.687 17 0", 2)],
};
static WIFI_OFF: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::WifiOff, origins: MID,
    parts: &[
        moves("M12 18l.01 0", 1), moves("M9.172 15.172a4 4 0 0 1 5.656 0", 1),
        moves("M6.343 12.343a7.963 7.963 0 0 1 3.864 -2.14m4.163 .155a7.965 7.965 0 0 1 3.287 2", 1),
        moves("M3.515 9.515a12 12 0 0 1 3.544 -2.455m3.101 -.92a12 12 0 0 1 10.325 3.374", 1),
        moves("M3 3l18 18", 0),
    ],
};
/// "No Internet": the globe with a slash.
static NO_INTERNET: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::WifiOff, origins: MID,
    parts: &[
        moves("M5.657 5.615a9 9 0 1 0 12.717 12.739m1.672 -2.322a9 9 0 0 0 -12.066 -12.084", 1),
        moves("M3.6 9h5.4m4 0h7.4", 1), moves("M3.6 15h11.4m4 0h1.4", 1),
        moves("M11.5 3a17.001 17.001 0 0 0 -1.493 3.022m-.847 3.145c-.68 4.027 .1 8.244 2.34 11.833", 1),
        moves("M12.5 3a16.982 16.982 0 0 1 2.549 8.005m-.207 3.818a16.979 16.979 0 0 1 -2.342 6.177", 1),
        moves("M3 3l18 18", 0),
    ],
};
static NETWORK: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Pulse, origins: MID,
    parts: &[
        moves("M6 9a6 6 0 1 0 12 0a6 6 0 0 0 -12 0", 0), moves("M12 3c1.333 .333 2 2.333 2 6s-.667 5.667 -2 6", 0),
        moves("M12 3c-1.333 .333 -2 2.333 -2 6s.667 5.667 2 6", 0), moves("M6 9h12", 0), moves("M3 20h7", 0), moves("M14 20h7", 0),
        moves("M10 20a2 2 0 1 0 4 0a2 2 0 0 0 -4 0", 0), moves("M12 15v3", 0),
    ],
};
static MOBILE: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Ring, origins: MID,
    parts: &[moves("M6 5a2 2 0 0 1 2 -2h8a2 2 0 0 1 2 2v14a2 2 0 0 1 -2 2h-8a2 2 0 0 1 -2 -2v-14z", 0), moves("M11 4h2", 0), moves("M12 17v.01", 0)],
};
static BATTERY_LOW: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::BatteryLow, origins: MID,
    parts: &[
        p("M6 7h11a2 2 0 0 1 2 2v.5a.5 .5 0 0 0 .5 .5a.5 .5 0 0 1 .5 .5v3a.5 .5 0 0 1 -.5 .5a.5 .5 0 0 0 -.5 .5v.5a2 2 0 0 1 -2 2h-11a2 2 0 0 1 -2 -2v-6a2 2 0 0 1 2 -2"),
        fades("M7 10l0 4", 0),
    ],
};
static SAVER: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Saver, origins: [(6.5, 16.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
    parts: &[
        p("M4 9a2 2 0 0 1 2 -2h11a2 2 0 0 1 2 2v.5a.5 .5 0 0 0 .5 .5a.5 .5 0 0 1 .5 .5v3a.5 .5 0 0 1 -.5 .5a.5 .5 0 0 0 -.5 .5v.5a2 2 0 0 1 -2 2h-5.5"),
        moves("M3 16.143c0 -2.84 2.09 -5.143 4.667 -5.143h2.333v.857c0 2.84 -2.09 5.143 -4.667 5.143h-2.333v-.857z", 0),
        moves("M3 20v-3", 0),
    ],
};
static BELL: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Bell, origins: [(12.0, 3.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
    parts: &[moves("M10 5a2 2 0 1 1 4 0a7 7 0 0 1 4 6v3a4 4 0 0 0 2 3h-16a4 4 0 0 0 2 -3v-3a7 7 0 0 1 4 -6", 0), moves("M9 17v1a3 3 0 0 0 6 0v-1", 0)],
};
static WALK: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Walk, origins: MID,
    parts: &[
        moves("M13 4m-1 0a1 1 0 1 0 2 0a1 1 0 1 0 -2 0", 0), moves("M7 21l3 -4", 0), moves("M16 21l-2 -4l-3 -3l1 -6", 0), moves("M6 12l2 -3l4 -1l3 3l3 1", 0),
    ],
};
static HEADPHONES: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Headphones, origins: [(12.0, 16.5), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
    parts: &[
        moves("M4 13m0 2a2 2 0 0 1 2 -2h1a2 2 0 0 1 2 2v3a2 2 0 0 1 -2 2h-1a2 2 0 0 1 -2 -2z", 0),
        moves("M15 13m0 2a2 2 0 0 1 2 -2h1a2 2 0 0 1 2 2v3a2 2 0 0 1 -2 2h-1a2 2 0 0 1 -2 -2z", 0),
        p("M4 15v-3a8 8 0 0 1 16 0v3"),
    ],
};
static SPEAKER: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::SpeakerCone, origins: [(12.0, 14.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)],
    parts: &[p("M5 3m0 2a2 2 0 0 1 2 -2h10a2 2 0 0 1 2 2v14a2 2 0 0 1 -2 2h-10a2 2 0 0 1 -2 -2z"), moves("M12 14m-3 0a3 3 0 1 0 6 0a3 3 0 1 0 -6 0", 0), p("M12 7l0 .01")],
};
static MOUSE: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::MouseWheel, origins: MID,
    parts: &[p("M6 3m0 4a4 4 0 0 1 4 -4h4a4 4 0 0 1 4 4v10a4 4 0 0 1 -4 4h-4a4 4 0 0 1 -4 -4z"), moves("M12 7l0 4", 0)],
};
static GAMEPAD: Icon = Icon {
    vb: 24.0, sw: 2.0, anim: Anim::Gamepad, origins: MID,
    parts: &[
        p("M12 5h3.5a5 5 0 0 1 0 10h-5.5l-4.015 4.227a2.3 2.3 0 0 1 -3.923 -2.035l1.634 -8.173a5 5 0 0 1 4.904 -4.019h3.4z"),
        p("M14 15l4.07 4.284a2.3 2.3 0 0 0 3.925 -2.023l-1.6 -8.232"),
        fades("M8 9v2", 0), fades("M7 10h2", 0), fades("M14 10h2", 0),
    ],
};

/// The line icon drawn in place of a banner glyph, if it has one.
pub fn lookup(glyph: &str) -> Option<&'static Icon> {
    Some(match glyph {
        ICON_BT => &BLUETOOTH,
        ICON_BRIGHTNESS => &SUN,
        ICON_PLUG | ICON_BATTERY => &CHARGING,
        ICON_UNPLUG => &PLUG,
        ICON_AIRPLANE => &PLANE,
        ICON_VOLUME => &VOLUME,
        ICON_MUTE => &MUTE,
        ICON_MIC => &MIC,
        ICON_MIC_OFF => &MIC_OFF,
        ICON_KEYBOARD => &KEYBOARD,
        ICON_WIFI => &WIFI,
        ICON_WIFI_OFF => &WIFI_OFF,
        ICON_NO_INTERNET => &NO_INTERNET,
        ICON_ETHERNET => &NETWORK,
        ICON_MOBILE | ICON_PHONE_DEVICE => &MOBILE,
        ICON_BATTERY_LOW => &BATTERY_LOW,
        ICON_SAVER => &SAVER,
        ICON_REMINDER | ICON_BELL => &BELL,
        ICON_WALK => &WALK,
        ICON_HEADPHONES => &HEADPHONES,
        ICON_SPEAKER => &SPEAKER,
        ICON_MOUSE => &MOUSE,
        ICON_GAMEPAD => &GAMEPAD,
        _ => return None,
    })
}

// ---- motion ----------------------------------------------------------------------------------

/// Seconds an icon moves for after its banner appears; still (as drawn) after that.
pub const DUR: f32 = 1.0;

/// How one group is moved: turn (degrees) and scale about the group's origin, then shift (grid
/// units), and its opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub rot: f32,
    pub scale: f32,
    pub dx: f32,
    pub dy: f32,
    pub alpha: f32,
}

impl Motion {
    pub const STILL: Motion = Motion { rot: 0.0, scale: 1.0, dx: 0.0, dy: 0.0, alpha: 1.0 };
}

fn lerp_in(t: f32, from: f32, len: f32) -> f32 {
    ((t - from) / len).clamp(0.0, 1.0)
}

/// 0 -> 1 -> 0 over `len` seconds from `from`.
fn bump(t: f32, from: f32, len: f32) -> f32 {
    (std::f32::consts::PI * lerp_in(t, from, len)).sin()
}

fn ease_out(x: f32) -> f32 {
    1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

/// The four groups of `anim` at `age` seconds.
pub fn motion(anim: Anim, age: f32) -> [Motion; 4] {
    let mut g = [Motion::STILL; 4];
    if age >= DUR {
        return g;
    }
    let t = age.max(0.0);
    let tau = std::f32::consts::TAU;
    let decay = (-4.0 * t).exp();
    let shake = (tau * 3.5 * t).sin() * decay;
    let scale = |peak: f32, from: f32, len: f32| 1.0 + (peak - 1.0) * bump(t, from, len);
    match anim {
        Anim::Bluetooth => {
            g[0].scale = scale(1.1, 0.25, 0.6);
            for i in 0..2 {
                let x = (t - 0.25 - i as f32 * 0.2) / 0.8;
                g[i + 1].scale = 1.0 + 0.5 * (std::f32::consts::PI * x.clamp(0.0, 1.0)).sin();
                g[i + 1].alpha = 1.0 - 0.5 * (std::f32::consts::PI * x.clamp(0.0, 1.0)).sin();
            }
        }
        Anim::Sun => {
            g[0].scale = scale(0.8, 0.25, 0.4);
            g[1].alpha = 1.0 - 0.6 * bump(t, 0.25, 0.5);
        }
        Anim::Charging => {
            for (i, m) in g.iter_mut().enumerate() {
                m.alpha = lerp_in(t, 0.1 + i as f32 * 0.15, 0.15);
            }
        }
        // Joined at first (the halves pushed together), then pulling apart to where they rest.
        Anim::Unplug => {
            let k = 1.0 - ease_out(t / 0.5);
            (g[0].dx, g[0].dy, g[1].dx, g[1].dy) = (2.0 * k, -2.0 * k, -2.0 * k, 2.0 * k);
            g[2].alpha = 1.0 - k;
        }
        Anim::Plane => {
            let v = bump(t, 0.0, 0.6);
            (g[0].dx, g[0].dy) = (0.5 * v, -0.5 * v);
            // Two streaks of wind sweep down and left across the icon, fading in and out.
            let (a, b) = (ease_out(t / 0.9), ease_out((t - 0.2) / 0.9));
            let streak = |x: f32| (std::f32::consts::PI * x).sin().max(0.0);
            (g[1].dx, g[1].dy, g[1].alpha) = (10.0 - 22.0 * a, -6.0 + 30.0 * a, streak(a));
            (g[2].dx, g[2].dy, g[2].alpha) = (26.0 - 26.0 * b, 8.0 + 18.0 * b, streak(b));
        }
        // The two sound waves appear one after the other.
        Anim::Volume => {
            g[0].alpha = lerp_in(t, 0.1, 0.15);
            g[1].alpha = lerp_in(t, 0.25, 0.15);
        }
        Anim::Mute => {
            g[0].scale = 0.5 + 0.5 * lerp_in(t, 0.1, 0.2) + 0.15 * bump(t, 0.3, 0.3);
            g[0].alpha = lerp_in(t, 0.1, 0.15);
        }
        Anim::Mic => g[0].scale = scale(1.12, 0.25, 0.5),
        Anim::MicOff => {
            g[0].scale = 0.5 + 0.5 * lerp_in(t, 0.15, 0.25);
            g[0].alpha = lerp_in(t, 0.15, 0.2);
            g[1].dx = 1.2 * shake;
        }
        Anim::Keyboard => {
            let press = bump(t, 0.2, 0.3);
            g[0].dy = 0.8 * press;
            g[0].scale = 1.0 - 0.08 * press;
        }
        Anim::Wifi => {
            for (i, m) in g.iter_mut().take(3).enumerate() {
                m.alpha = lerp_in(t, 0.1 + i as f32 * 0.12, 0.12);
            }
        }
        Anim::WifiOff => {
            g[0].scale = 0.5 + 0.5 * lerp_in(t, 0.15, 0.25);
            g[0].alpha = lerp_in(t, 0.15, 0.2);
            g[1].dx = 1.0 * shake;
        }
        Anim::Pulse => g[0].scale = scale(1.1, 0.25, 0.5),
        Anim::Ring => g[0].rot = 14.0 * shake,
        // Two blinks of the single remaining bar.
        Anim::BatteryLow => g[0].alpha = 1.0 - 0.9 * (tau * 2.0 * lerp_in(t, 0.2, 0.8)).sin().abs(),
        Anim::Saver => g[0].scale = scale(1.2, 0.25, 0.5),
        Anim::Bell => g[0].rot = 24.0 * (-2.5 * t).exp() * (tau * 2.5 * t).sin(),
        Anim::Walk => g[0].dy = -1.0 * decay * (tau * 2.0 * t).sin().abs(),
        Anim::Headphones => g[0].scale = scale(1.12, 0.25, 0.5),
        Anim::SpeakerCone => g[0].scale = scale(1.18, 0.25, 0.4),
        Anim::MouseWheel => g[0].dy = 1.5 * bump(t, 0.2, 0.5),
        Anim::Gamepad => g[0].alpha = 1.0 - 0.7 * (tau * 1.5 * lerp_in(t, 0.2, 0.7)).sin().abs(),
    }
    g
}

/// The whole icon pops in: scale and opacity at `age`.
pub fn pop(age: f32) -> (f32, f32) {
    let x = (age / 0.3).clamp(0.0, 1.0);
    let (c1, y) = (1.3, x - 1.0);
    let back = 1.0 + (c1 + 1.0) * y * y * y + c1 * y * y;
    (0.75 + 0.25 * back, (age / 0.15).clamp(0.0, 1.0))
}

/// Water drop (drawn by the renderer itself): pops in, then hops.
pub fn drop_motion(age: f32) -> Motion {
    if age >= DUR {
        return Motion::STILL;
    }
    Motion { dy: -1.4 * (-4.0 * age).exp() * (std::f32::consts::TAU * 2.5 * age).sin().abs(), ..Motion::STILL }
}

// ---- SVG path data -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(f32, f32),
    Line(f32, f32),
    /// Control 1, control 2, end.
    Cubic([f32; 6]),
    /// Radii, x-axis rotation in degrees, large-arc flag, sweep flag (clockwise), end.
    Arc { rx: f32, ry: f32, rot: f32, large: bool, sweep: bool, x: f32, y: f32 },
    Close,
}

struct Scan<'a> {
    s: &'a [u8],
    i: usize,
}

impl Scan<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b',' | b'\n' | b'\t' | b'\r') {
            self.i += 1;
        }
    }

    /// A number: optional sign, digits, optional fraction ("-.5", "1.5", ".01"); a second '.' starts the next number.
    fn num(&mut self) -> Option<f32> {
        self.skip();
        let start = self.i;
        if self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+') {
            self.i += 1;
        }
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        if self.i < self.s.len() && self.s[self.i] == b'.' {
            self.i += 1;
            while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                self.i += 1;
            }
        }
        std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
    }

    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let f = match self.s.get(self.i)? {
            b'0' => false,
            b'1' => true,
            _ => return None,
        };
        self.i += 1;
        Some(f)
    }
}

/// Parse SVG path data (M L H V C S A Z, absolute or relative; quadratic curves are not used by
/// these icons and give None) into absolute segments.
pub fn parse(d: &str) -> Option<Vec<Seg>> {
    let mut sc = Scan { s: d.as_bytes(), i: 0 };
    let (mut cx, mut cy, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut out = Vec::new();
    let mut cmd = b' ';
    loop {
        sc.skip();
        let Some(&c) = sc.s.get(sc.i) else { break };
        if c.is_ascii_alphabetic() {
            cmd = c;
            sc.i += 1;
        } else if cmd == b' ' {
            return None;
        }
        let rel = cmd.is_ascii_lowercase();
        let at = |rel: bool, base: f32, v: f32| if rel { base + v } else { v };
        match cmd.to_ascii_uppercase() {
            b'Z' => {
                out.push(Seg::Close);
                (cx, cy) = (sx, sy);
                last_ctrl = None;
            }
            b'M' => {
                let x = at(rel, cx, sc.num()?);
                let y = at(rel, cy, sc.num()?);
                out.push(Seg::Move(x, y));
                (cx, cy, sx, sy) = (x, y, x, y);
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
            }
            b'L' => {
                let x = at(rel, cx, sc.num()?);
                let y = at(rel, cy, sc.num()?);
                out.push(Seg::Line(x, y));
                (cx, cy) = (x, y);
                last_ctrl = None;
            }
            b'H' => {
                let x = at(rel, cx, sc.num()?);
                out.push(Seg::Line(x, cy));
                cx = x;
                last_ctrl = None;
            }
            b'V' => {
                let y = at(rel, cy, sc.num()?);
                out.push(Seg::Line(cx, y));
                cy = y;
                last_ctrl = None;
            }
            b'C' => {
                let (x1, y1) = (at(rel, cx, sc.num()?), at(rel, cy, sc.num()?));
                let (x2, y2) = (at(rel, cx, sc.num()?), at(rel, cy, sc.num()?));
                let (x, y) = (at(rel, cx, sc.num()?), at(rel, cy, sc.num()?));
                out.push(Seg::Cubic([x1, y1, x2, y2, x, y]));
                (cx, cy, last_ctrl) = (x, y, Some((x2, y2)));
            }
            b'S' => {
                let (x2, y2) = (at(rel, cx, sc.num()?), at(rel, cy, sc.num()?));
                let (x, y) = (at(rel, cx, sc.num()?), at(rel, cy, sc.num()?));
                let (x1, y1) = last_ctrl.map_or((cx, cy), |(a, b)| (2.0 * cx - a, 2.0 * cy - b));
                out.push(Seg::Cubic([x1, y1, x2, y2, x, y]));
                (cx, cy, last_ctrl) = (x, y, Some((x2, y2)));
            }
            b'A' => {
                let (rx, ry, rot) = (sc.num()?, sc.num()?, sc.num()?);
                let (large, sweep) = (sc.flag()?, sc.flag()?);
                let (x, y) = (at(rel, cx, sc.num()?), at(rel, cy, sc.num()?));
                out.push(Seg::Arc { rx, ry, rot, large, sweep, x, y });
                (cx, cy) = (x, y);
                last_ctrl = None;
            }
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines_relative_and_absolute() {
        assert_eq!(parse("M7 8l10 8l-5 4"), Some(vec![Seg::Move(7.0, 8.0), Seg::Line(17.0, 16.0), Seg::Line(12.0, 20.0)]));
        assert_eq!(parse("M3 12h1m8 -9v1"), Some(vec![Seg::Move(3.0, 12.0), Seg::Line(4.0, 12.0), Seg::Move(12.0, 3.0), Seg::Line(12.0, 4.0)]));
        assert_eq!(parse("M0 0L5 5 10 0z"), Some(vec![Seg::Move(0.0, 0.0), Seg::Line(5.0, 5.0), Seg::Line(10.0, 0.0), Seg::Close]), "implicit lineto, close");
    }

    #[test]
    fn parses_packed_numbers_and_arcs() {
        // ".5.5" is two numbers, "-.5" is one.
        assert_eq!(parse("M10 10l.5.5l-.5 1"), Some(vec![Seg::Move(10.0, 10.0), Seg::Line(10.5, 10.5), Seg::Line(10.0, 11.5)]));
        let a = parse("M0 0a4 4 0 1 0 8 0").unwrap();
        assert_eq!(a[1], Seg::Arc { rx: 4.0, ry: 4.0, rot: 0.0, large: true, sweep: false, x: 8.0, y: 0.0 });
        // 'm' after the first move: relative to the current point, then implicit relative lineto.
        let m = parse("M12 12m-4 0a4 4 0 1 0 8 0").unwrap();
        assert_eq!(m[1], Seg::Move(8.0, 12.0));
    }

    #[test]
    fn parses_curves_and_rejects_what_it_cannot_draw() {
        let c = parse("M0 0c1 2 3 4 5 6").unwrap();
        assert_eq!(c[1], Seg::Cubic([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        let s = parse("M0 0C1 1 2 2 3 3S5 5 6 6").unwrap();
        assert_eq!(s[2], Seg::Cubic([4.0, 4.0, 5.0, 5.0, 6.0, 6.0]), "first control reflected");
        assert_eq!(parse("M0 0Q1 1 2 2"), None, "quadratics are not drawn");
        assert_eq!(parse("5 5"), None, "numbers with no command");
        assert_eq!(parse("M0"), None, "missing coordinate");
    }

    #[test]
    fn every_icon_path_parses_and_groups_exist() {
        let glyphs = [ICON_BT, ICON_BRIGHTNESS, ICON_PLUG, ICON_BATTERY, ICON_UNPLUG, ICON_AIRPLANE, ICON_VOLUME, ICON_MUTE, ICON_MIC, ICON_MIC_OFF,
            ICON_KEYBOARD, ICON_WIFI, ICON_WIFI_OFF, ICON_NO_INTERNET, ICON_ETHERNET, ICON_MOBILE, ICON_BATTERY_LOW, ICON_SAVER, ICON_REMINDER, ICON_BELL,
            ICON_WALK, ICON_HEADPHONES, ICON_SPEAKER, ICON_PHONE_DEVICE, ICON_MOUSE, ICON_GAMEPAD];
        for glyph in glyphs {
            let icon = lookup(glyph).unwrap_or_else(|| panic!("{glyph:?} has no line icon"));
            for part in icon.parts {
                let segs = parse(part.d).unwrap_or_else(|| panic!("{glyph:?}: {:?} does not parse", part.d));
                assert!(matches!(segs.first(), Some(Seg::Move(..))), "{:?} starts with a move", part.d);
                assert!(part.g == NONE || part.g < 4, "{:?}", part.d);
                assert!(part.a == NONE || part.a < 4, "{:?}", part.d);
            }
        }
        assert!(lookup(ICON_DROP).is_none(), "the drop is drawn by the renderer");
    }

    const ALL: [Anim; 22] = [
        Anim::Bluetooth, Anim::Sun, Anim::Charging, Anim::Unplug, Anim::Plane, Anim::Volume, Anim::Mute, Anim::Mic, Anim::MicOff, Anim::Keyboard,
        Anim::Wifi, Anim::WifiOff, Anim::Pulse, Anim::Ring, Anim::BatteryLow, Anim::Saver, Anim::Bell, Anim::Walk, Anim::Headphones,
        Anim::SpeakerCone, Anim::MouseWheel, Anim::Gamepad,
    ];

    #[test]
    fn every_motion_ends_as_drawn_and_stays_in_bounds() {
        for anim in ALL {
            assert_eq!(motion(anim, DUR), [Motion::STILL; 4], "{anim:?} rests when over");
            assert_eq!(motion(anim, 9.0), [Motion::STILL; 4]);
            for i in 0..=100 {
                for m in motion(anim, i as f32 * DUR / 100.0) {
                    assert!(m.rot.is_finite() && m.scale > 0.0 && m.scale <= 1.55 && (0.0..=1.0).contains(&m.alpha) && m.dx.abs() < 30.0 && m.dy.abs() < 30.0, "{anim:?} t={i}: {m:?}");
                }
            }
        }
    }

    #[test]
    fn motions_do_what_they_say() {
        let range = |anim: Anim, g: usize, f: fn(&Motion) -> f32| {
            let v: Vec<f32> = (0..100).map(|i| f(&motion(anim, i as f32 * 0.01)[g])).collect();
            (v.iter().cloned().fold(f32::MAX, f32::min), v.iter().cloned().fold(f32::MIN, f32::max))
        };
        // Bluetooth: the symbol swells, the dots swell and dim one after the other.
        assert!(range(Anim::Bluetooth, 0, |m| m.scale).1 > 1.05);
        let dot = |i: usize, t: f32| motion(Anim::Bluetooth, t)[i].scale;
        assert!(dot(1, 0.45) > dot(2, 0.45), "first dot peaks before the second");
        // The charging bars fill in order and all end lit.
        let bars = motion(Anim::Charging, 0.3);
        assert!(bars[0].alpha > bars[1].alpha && bars[1].alpha >= bars[2].alpha && bars[2].alpha >= bars[3].alpha, "{bars:?}");
        assert_eq!(motion(Anim::Charging, 0.0)[0].alpha, 0.0);
        // Wi-Fi arcs appear one after the other; volume waves too.
        let w = motion(Anim::Wifi, 0.3);
        assert!(w[0].alpha > w[1].alpha && w[1].alpha > w[2].alpha, "{w:?}");
        let v = motion(Anim::Volume, 0.2);
        assert!(v[0].alpha > v[1].alpha, "{v:?}");
        // Off / muted: the rest of the icon shakes, both ways; the mute cross pops in.
        let (lo, hi) = range(Anim::MicOff, 1, |m| m.dx);
        assert!(lo < -0.2 && hi > 0.2, "{lo} {hi}");
        assert!(motion(Anim::Mute, 0.0)[0].scale < 0.6 && motion(Anim::Mute, 0.5)[0].scale > 0.95);
        // Unplug: the halves start pushed together and pull apart; the legs come back.
        let start = motion(Anim::Unplug, 0.0);
        assert!(start[0].dx > 1.0 && start[1].dx < -1.0 && start[2].alpha < 0.1, "{start:?}");
        assert_eq!(motion(Anim::Unplug, 0.6)[0].dx, 0.0);
        // A key goes down; a low battery's bar blinks; the bell swings both ways from its top.
        assert!(range(Anim::Keyboard, 0, |m| m.dy).1 > 0.5);
        assert!(range(Anim::BatteryLow, 0, |m| m.alpha).0 < 0.3);
        let (lo, hi) = range(Anim::Bell, 0, |m| m.rot);
        assert!(lo < -5.0 && hi > 5.0);
        // The plane's wind streaks cross the icon and fade out.
        assert!(range(Anim::Plane, 1, |m| m.alpha).1 > 0.9 && motion(Anim::Plane, 0.99)[1].alpha < 0.01);
    }

    #[test]
    fn pop_starts_small_and_ends_full() {
        let (s, a) = pop(0.0);
        assert!(s < 0.8 && a == 0.0);
        let (s, a) = pop(0.5);
        assert!((s - 1.0).abs() < 1e-5 && a == 1.0);
        assert!(drop_motion(0.0) == Motion::STILL && drop_motion(0.1).dy < 0.0 && drop_motion(DUR) == Motion::STILL);
    }
}

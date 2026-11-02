//! Curated low-chroma palettes, baked from OKLCH into sRGB.
//! Ramps run from page/surface/hover/active to faint/secondary/body/primary text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum ThemePreset {
    Dune = 1,
    Grove,
    Tide,
    Rosewood,
    Ember,
    Midnight,
    Aubergine,
    Olive,
    Porcelain,
    Cocoa,
    Iris,
    Onyx,
    Obsidian,
    Soot,
    Pine,
    Oxblood,
    Eclipse,
    Vercel,
    #[default]
    #[serde(other)]
    Graphite = 0,
}

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub neutral: [u32; 8],
    pub shell: u32,
    pub composer: u32,
    pub popup: u32,
    pub accent: u32,
    pub primary: u32,
    pub primary_foreground: u32,
}

impl ThemePreset {
    pub const ALL: [Self; 19] = [
        Self::Graphite,
        Self::Dune,
        Self::Grove,
        Self::Tide,
        Self::Rosewood,
        Self::Ember,
        Self::Midnight,
        Self::Aubergine,
        Self::Olive,
        Self::Porcelain,
        Self::Cocoa,
        Self::Iris,
        Self::Onyx,
        Self::Obsidian,
        Self::Soot,
        Self::Pine,
        Self::Oxblood,
        Self::Eclipse,
        Self::Vercel,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Graphite => "Graphite",
            Self::Dune => "Dune",
            Self::Grove => "Grove",
            Self::Tide => "Tide",
            Self::Rosewood => "Rosewood",
            Self::Ember => "Ember",
            Self::Midnight => "Midnight",
            Self::Aubergine => "Aubergine",
            Self::Olive => "Olive",
            Self::Porcelain => "Porcelain",
            Self::Cocoa => "Cocoa",
            Self::Iris => "Iris",
            Self::Vercel => "Vercel",
            Self::Onyx => "Onyx",
            Self::Obsidian => "Obsidian",
            Self::Soot => "Soot",
            Self::Pine => "Pine",
            Self::Oxblood => "Oxblood",
            Self::Eclipse => "Eclipse",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Graphite => "The original, quietly neutral",
            Self::Dune => "Sandstone and burnished brass",
            Self::Grove => "Forest shade and soft sage",
            Self::Tide => "Sea slate and weathered teal",
            Self::Rosewood => "Dusty rose and warm timber",
            Self::Ember => "Charcoal clay and terracotta",
            Self::Midnight => "Deep navy and silver blue",
            Self::Aubergine => "Smoky plum and muted mauve",
            Self::Olive => "Earthy khaki and muted gold",
            Self::Porcelain => "Cool chalk and soft ink",
            Self::Cocoa => "Roasted brown and warm cream",
            Self::Iris => "Lavender slate and soft violet",
            Self::Vercel => "Pitch black. Pure focus.",
            Self::Onyx => "Crisp black, chalk white, no color",
            Self::Obsidian => "Blue-black with a steel-blue edge",
            Self::Soot => "Warm black with a trace of amber",
            Self::Pine => "Near-black with deep forest undertones",
            Self::Oxblood => "Ink black with a muted wine tint",
            Self::Eclipse => "Black violet with a silver-lilac accent",
        }
    }
    pub fn swatches(self, light: bool) -> [u32; 4] {
        let p = self.palette(light);
        [p.shell, p.neutral[2], p.accent, p.neutral[7]]
    }
    pub(super) fn palette(self, light: bool) -> Palette {
        match (self, light) {
            (Self::Graphite, false) => Palette {
                neutral: [
                    0x212225, 0x27282c, 0x2b2c30, 0x34353a, 0x83837f, 0xa3a39f, 0xdcdcd8, 0xececea,
                ],
                shell: 0x1d1e21,
                composer: 0x2a2b2f,
                popup: 0x28292d,
                accent: 0x8fb4e6,
                primary: 0x3b6db3,
                primary_foreground: 0xffffff,
            },
            (Self::Graphite, true) => Palette {
                neutral: [
                    0xfbfaf8, 0xf1efeb, 0xe8e5df, 0xddd9d1, 0x76736c, 0x625f59, 0x393732, 0x2a2926,
                ],
                shell: 0xf7f6f3,
                composer: 0xf1efeb,
                popup: 0xf6f5f2,
                accent: 0x2f5f9e,
                primary: 0x2f5f9e,
                primary_foreground: 0xffffff,
            },
            (Self::Dune, false) => Palette {
                neutral: [
                    0x272116, 0x2f281d, 0x362f24, 0x433c30, 0x968e80, 0xb2aa9c, 0xdfd6c8, 0xf3eadb,
                ],
                shell: 0x221c11,
                composer: 0x312a1f,
                popup: 0x30291e,
                accent: 0xd9af75,
                primary: 0x7b5518,
                primary_foreground: 0xffffff,
            },
            (Self::Dune, true) => Palette {
                neutral: [
                    0xfdfaf4, 0xf3f0ea, 0xece3d5, 0xdcd3c4, 0x675f53, 0x534c40, 0x312a1f, 0x272116,
                ],
                shell: 0xf9f4ee,
                composer: 0xf3eadb,
                popup: 0xfaf6f0,
                accent: 0x6c4701,
                primary: 0x7b5518,
                primary_foreground: 0xffffff,
            },
            (Self::Grove, false) => Palette {
                neutral: [
                    0x1a251c, 0x212c23, 0x28342a, 0x354137, 0x869388, 0xa1afa3, 0xcddcd0, 0xe1f0e3,
                ],
                shell: 0x152017,
                composer: 0x232f25,
                popup: 0x222d24,
                accent: 0x8ec899,
                primary: 0x346c42,
                primary_foreground: 0xffffff,
            },
            (Self::Grove, true) => Palette {
                neutral: [
                    0xf6fcf7, 0xedf2ed, 0xdae9dd, 0xcad9cc, 0x58645a, 0x445147, 0x232f25, 0x1a251c,
                ],
                shell: 0xf1f7f2,
                composer: 0xe1f0e3,
                popup: 0xf3f9f4,
                accent: 0x255d34,
                primary: 0x346c42,
                primary_foreground: 0xffffff,
            },
            (Self::Tide, false) => Palette {
                neutral: [
                    0x142529, 0x1c2c30, 0x233338, 0x2f4045, 0x809398, 0x9bafb4, 0xc7dce1, 0xdbeff5,
                ],
                shell: 0x102024,
                composer: 0x1e2e32,
                popup: 0x1d2d31,
                accent: 0x6bc8d3,
                primary: 0x006c76,
                primary_foreground: 0xffffff,
            },
            (Self::Tide, true) => Palette {
                neutral: [
                    0xf4fcfd, 0xeaf2f3, 0xd4e9ee, 0xc4d8de, 0x526469, 0x3f5055, 0x1e2e32, 0x142529,
                ],
                shell: 0xeef7f9,
                composer: 0xdbeff5,
                popup: 0xf0f8fb,
                accent: 0x005d67,
                primary: 0x006c76,
                primary_foreground: 0xffffff,
            },
            (Self::Rosewood, false) => Palette {
                neutral: [
                    0x2d1d1e, 0x342425, 0x3c2b2c, 0x493839, 0x9e898a, 0xbaa5a6, 0xe8d1d2, 0xfce5e6,
                ],
                shell: 0x281819,
                composer: 0x372627,
                popup: 0x362526,
                accent: 0xeba0a5,
                primary: 0x89474d,
                primary_foreground: 0xffffff,
            },
            (Self::Rosewood, true) => Palette {
                neutral: [
                    0xfff8f8, 0xf5eeee, 0xf5dedf, 0xe4cecf, 0x6e5b5c, 0x5a4849, 0x372627, 0x2d1d1e,
                ],
                shell: 0xfcf3f3,
                composer: 0xfce5e6,
                popup: 0xfdf4f5,
                accent: 0x793940,
                primary: 0x89474d,
                primary_foreground: 0xffffff,
            },
            (Self::Ember, false) => Palette {
                neutral: [
                    0x2b1e18, 0x332520, 0x3b2d27, 0x483a33, 0x9c8b84, 0xb8a79f, 0xe5d3cb, 0xf9e7df,
                ],
                shell: 0x261a14,
                composer: 0x352822,
                popup: 0x342721,
                accent: 0xe8a587,
                primary: 0x874c30,
                primary_foreground: 0xffffff,
            },
            (Self::Ember, true) => Palette {
                neutral: [
                    0xfff9f6, 0xf5efec, 0xf3e0d8, 0xe2d0c8, 0x6c5d56, 0x584943, 0x352822, 0x2b1e18,
                ],
                shell: 0xfbf3f0,
                composer: 0xf9e7df,
                popup: 0xfcf5f2,
                accent: 0x773e22,
                primary: 0x874c30,
                primary_foreground: 0xffffff,
            },
            (Self::Midnight, false) => Palette {
                neutral: [
                    0x182232, 0x1f293a, 0x263142, 0x323e50, 0x8390a5, 0x9eacc1, 0xcad8ef, 0xdeecff,
                ],
                shell: 0x131d2d,
                composer: 0x212c3d,
                popup: 0x202a3c,
                accent: 0x96b9f1,
                primary: 0x3f5d90,
                primary_foreground: 0xffffff,
            },
            (Self::Midnight, true) => Palette {
                neutral: [
                    0xf7faff, 0xedf0f6, 0xd7e6fd, 0xc7d5ec, 0x556175, 0x424e61, 0x212c3d, 0x182232,
                ],
                shell: 0xf1f5fc,
                composer: 0xdeecff,
                popup: 0xf3f7fd,
                accent: 0x314f81,
                primary: 0x3f5d90,
                primary_foreground: 0xffffff,
            },
            (Self::Aubergine, false) => Palette {
                neutral: [
                    0x271e29, 0x2f2531, 0x362c39, 0x433946, 0x978a99, 0xb2a6b5, 0xdfd2e2, 0xf3e6f6,
                ],
                shell: 0x221925,
                composer: 0x312733,
                popup: 0x302632,
                accent: 0xd1a5db,
                primary: 0x744c7d,
                primary_foreground: 0xffffff,
            },
            (Self::Aubergine, true) => Palette {
                neutral: [
                    0xfdf8fe, 0xf3eef3, 0xeddff0, 0xdccfdf, 0x675c6a, 0x544956, 0x312733, 0x271e29,
                ],
                shell: 0xf8f3f9,
                composer: 0xf3e6f6,
                popup: 0xfaf5fb,
                accent: 0x653e6e,
                primary: 0x744c7d,
                primary_foreground: 0xffffff,
            },
            (Self::Olive, false) => Palette {
                neutral: [
                    0x222315, 0x292a1c, 0x313223, 0x3e3e30, 0x909180, 0xabac9b, 0xd8d9c7, 0xeceddb,
                ],
                shell: 0x1d1e11,
                composer: 0x2c2c1f,
                popup: 0x2b2b1d,
                accent: 0xbabd78,
                primary: 0x61621c,
                primary_foreground: 0xffffff,
            },
            (Self::Olive, true) => Palette {
                neutral: [
                    0xfafbf5, 0xf0f1eb, 0xe5e6d4, 0xd5d6c4, 0x616252, 0x4e4e3f, 0x2c2c1f, 0x222315,
                ],
                shell: 0xf5f6ee,
                composer: 0xeceddb,
                popup: 0xf7f7f1,
                accent: 0x535408,
                primary: 0x61621c,
                primary_foreground: 0xffffff,
            },
            (Self::Porcelain, false) => Palette {
                neutral: [
                    0x1f2224, 0x26292b, 0x2d3133, 0x3a3e40, 0x8c9092, 0xa7abae, 0xd4d8db, 0xe7ecef,
                ],
                shell: 0x1a1d1f,
                composer: 0x292c2e,
                popup: 0x272b2d,
                accent: 0x81bfeb,
                primary: 0x24648b,
                primary_foreground: 0xffffff,
            },
            (Self::Porcelain, true) => Palette {
                neutral: [
                    0xf6fbfe, 0xecf1f4, 0xe1e5e8, 0xd1d5d8, 0x5d6163, 0x4a4e50, 0x292c2e, 0x1f2224,
                ],
                shell: 0xf1f6f9,
                composer: 0xe7ecef,
                popup: 0xf3f7fa,
                accent: 0x12557c,
                primary: 0x24648b,
                primary_foreground: 0xffffff,
            },
            (Self::Cocoa, false) => Palette {
                neutral: [
                    0x2a1f14, 0x32261b, 0x3a2e22, 0x473a2f, 0x9b8c7f, 0xb7a89a, 0xe4d4c6, 0xf8e8da,
                ],
                shell: 0x251a10,
                composer: 0x35291e,
                popup: 0x33281c,
                accent: 0xe2aa7c,
                primary: 0x825023,
                primary_foreground: 0xffffff,
            },
            (Self::Cocoa, true) => Palette {
                neutral: [
                    0xfef9f5, 0xf4efeb, 0xf1e1d3, 0xe1d1c3, 0x6b5e51, 0x574a3e, 0x35291e, 0x2a1f14,
                ],
                shell: 0xfaf4ee,
                composer: 0xf8e8da,
                popup: 0xfbf6f1,
                accent: 0x734212,
                primary: 0x825023,
                primary_foreground: 0xffffff,
            },
            (Self::Iris, false) => Palette {
                neutral: [
                    0x211f2f, 0x282736, 0x302e3e, 0x3d3b4c, 0x8e8da0, 0xaaa8bc, 0xd7d5ea, 0xeae8fe,
                ],
                shell: 0x1d1b2a,
                composer: 0x2b2939,
                popup: 0x2a2838,
                accent: 0xb6aeee,
                primary: 0x5d548d,
                primary_foreground: 0xffffff,
            },
            (Self::Iris, true) => Palette {
                neutral: [
                    0xfaf9ff, 0xf0eff5, 0xe4e2f7, 0xd3d1e7, 0x605e70, 0x4c4b5c, 0x2b2939, 0x211f2f,
                ],
                shell: 0xf5f4fc,
                composer: 0xeae8fe,
                popup: 0xf6f6fd,
                accent: 0x50467d,
                primary: 0x5d548d,
                primary_foreground: 0xffffff,
            },
            (Self::Onyx, false) => Palette {
                neutral: [
                    0x0d0d0d, 0x161616, 0x1f1f1f, 0x2e2e2e, 0x8f8f8f, 0xababab, 0xdedede, 0xf5f5f5,
                ],
                shell: 0x060606,
                composer: 0x181818,
                popup: 0x171717,
                accent: 0xbebebe,
                primary: 0xe8e8e8,
                primary_foreground: 0x151515,
            },
            (Self::Onyx, true) => Palette {
                neutral: [
                    0xfcfcfc, 0xf2f2f2, 0xe8e8e8, 0xd7d7d7, 0x606060, 0x4d4d4d, 0x292929, 0x1b1b1b,
                ],
                shell: 0xf7f7f7,
                composer: 0xededed,
                popup: 0xf8f8f8,
                accent: 0x505050,
                primary: 0x2e2e2e,
                primary_foreground: 0xffffff,
            },
            (Self::Obsidian, false) => Palette {
                neutral: [
                    0x080e15, 0x11161e, 0x1a2028, 0x282e37, 0x88909a, 0xa3acb6, 0xd6dfea, 0xf2f5f9,
                ],
                shell: 0x03060c,
                composer: 0x131921,
                popup: 0x12181f,
                accent: 0xa0c0eb,
                primary: 0x425f85,
                primary_foreground: 0xffffff,
            },
            (Self::Obsidian, true) => Palette {
                neutral: [
                    0xf9fcff, 0xeff2f6, 0xe0e9f4, 0xd0d8e4, 0x5a616b, 0x474e57, 0x232932, 0x151b23,
                ],
                shell: 0xf3f7fc,
                composer: 0xe5eef9,
                popup: 0xf5f9fe,
                accent: 0x355176,
                primary: 0x425f85,
                primary_foreground: 0xffffff,
            },
            (Self::Soot, false) => Palette {
                neutral: [
                    0x110c08, 0x1a1510, 0x231e19, 0x322d27, 0x948e87, 0xb0aaa3, 0xe3ddd6, 0xf8f4f1,
                ],
                shell: 0x080503,
                composer: 0x1c1712,
                popup: 0x1b1611,
                accent: 0xdbb68d,
                primary: 0x77562f,
                primary_foreground: 0xffffff,
            },
            (Self::Soot, true) => Palette {
                neutral: [
                    0xfefbf7, 0xf4f1ed, 0xede7df, 0xddd6cf, 0x655f59, 0x514c46, 0x2d2823, 0x1e1a15,
                ],
                shell: 0xfaf6f1,
                composer: 0xf2ece4,
                popup: 0xfcf8f3,
                accent: 0x684821,
                primary: 0x77562f,
                primary_foreground: 0xffffff,
            },
            (Self::Pine, false) => Palette {
                neutral: [
                    0x07100a, 0x101912, 0x19221c, 0x27312a, 0x87938a, 0xa2aea6, 0xd5e2d9, 0xf2f6f3,
                ],
                shell: 0x020704,
                composer: 0x121b15,
                popup: 0x111a14,
                accent: 0x9accaa,
                primary: 0x3b694c,
                primary_foreground: 0xffffff,
            },
            (Self::Pine, true) => Palette {
                neutral: [
                    0xf9fdfa, 0xeff3f0, 0xdfece3, 0xcfdbd2, 0x59645c, 0x465049, 0x222c25, 0x141d17,
                ],
                shell: 0xf3f8f4,
                composer: 0xe4f1e8,
                popup: 0xf4faf6,
                accent: 0x2d5b3f,
                primary: 0x3b694c,
                primary_foreground: 0xffffff,
            },
            (Self::Oxblood, false) => Palette {
                neutral: [
                    0x140a0b, 0x1e1313, 0x271c1c, 0x362a2b, 0x9a8b8c, 0xb6a6a7, 0xead9da, 0xf9f3f4,
                ],
                shell: 0x0b0404,
                composer: 0x201516,
                popup: 0x1f1414,
                accent: 0xe7acaf,
                primary: 0x804d51,
                primary_foreground: 0xffffff,
            },
            (Self::Oxblood, true) => Palette {
                neutral: [
                    0xfffafa, 0xf6f0f0, 0xf4e3e4, 0xe3d3d4, 0x6a5c5d, 0x57494a, 0x312526, 0x221718,
                ],
                shell: 0xfcf5f5,
                composer: 0xf9e8e9,
                popup: 0xfef6f7,
                accent: 0x713f43,
                primary: 0x804d51,
                primary_foreground: 0xffffff,
            },
            (Self::Eclipse, false) => Palette {
                neutral: [
                    0x0e0b14, 0x17141d, 0x211d27, 0x2f2c36, 0x918d99, 0xaca8b5, 0xe0dbe9, 0xf6f4f9,
                ],
                shell: 0x07050b,
                composer: 0x1a1620,
                popup: 0x18151f,
                accent: 0xc4b4e5,
                primary: 0x64547f,
                primary_foreground: 0xffffff,
            },
            (Self::Eclipse, true) => Palette {
                neutral: [
                    0xfcfbff, 0xf2f1f5, 0xe9e5f3, 0xd9d5e2, 0x625e6a, 0x4e4b56, 0x2a2731, 0x1c1922,
                ],
                shell: 0xf7f6fb,
                composer: 0xeeeaf8,
                popup: 0xf9f7fd,
                accent: 0x564771,
                primary: 0x64547f,
                primary_foreground: 0xffffff,
            },
            (Self::Vercel, false) => Palette {
                neutral: [
                    0x000000, 0x0a0a0a, 0x171717, 0x262626, 0x929292, 0xb5b5b5, 0xededed, 0xffffff,
                ],
                shell: 0x000000,
                composer: 0x0a0a0a,
                popup: 0x0a0a0a,
                accent: 0xffffff,
                primary: 0xffffff,
                primary_foreground: 0x000000,
            },
            (Self::Vercel, true) => Palette {
                neutral: [
                    0xffffff, 0xf5f5f5, 0xebebeb, 0xe0e0e0, 0x737373, 0x595959, 0x262626, 0x0a0a0a,
                ],
                shell: 0xfafafa,
                composer: 0xf5f5f5,
                popup: 0xffffff,
                accent: 0x171717,
                primary: 0x171717,
                primary_foreground: 0xffffff,
            },
        }
    }
}

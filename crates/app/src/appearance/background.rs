//! Background choice is independent of the palette; a theme only sets the first default.
use super::ThemePreset;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum BackgroundAnimation {
    Off = 1,
    Life,
    #[serde(alias = "waves")]
    Liquid,
    Contours,
    Drift,
    #[serde(alias = "orbit")]
    FlutedGlass,
    Curtains,
    Rain,
    Clouds,
    FallingLeaves,
    Pudding,
    WindowCurtains,
    Seafloor,
}

impl BackgroundAnimation {
    pub const ALL: [Self; 13] = [
        Self::Off,
        Self::Life,
        Self::Liquid,
        Self::Contours,
        Self::Drift,
        Self::FlutedGlass,
        Self::Curtains,
        Self::Rain,
        Self::Clouds,
        Self::FallingLeaves,
        Self::Pudding,
        Self::WindowCurtains,
        Self::Seafloor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Life => "Life",
            Self::Liquid => "Liquid",
            Self::Contours => "Contours",
            Self::Drift => "Drift",
            Self::FlutedGlass => "Fluted Glass",
            Self::Curtains => "Curtains",
            Self::Rain => "Rain",
            Self::Clouds => "Clouds",
            Self::FallingLeaves => "Falling Leaves",
            Self::Pudding => "Pudding",
            Self::WindowCurtains => "Window Curtains",
            Self::Seafloor => "Seafloor",
        }
    }

    /// What someone with no saved choice starts on: Vercel stays still, others show Life.
    pub fn starting_for(theme: ThemePreset) -> Self {
        if theme == ThemePreset::Vercel {
            Self::Off
        } else {
            Self::Life
        }
    }
    pub fn from_index(index: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|choice| *choice as u8 == index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_round_trips_and_the_starting_choice_preserves_vercel_focus() {
        for choice in BackgroundAnimation::ALL {
            assert_eq!(BackgroundAnimation::from_index(choice as u8), Some(choice));
        }
        assert_eq!(BackgroundAnimation::from_index(0), None);
        for theme in ThemePreset::ALL {
            assert_eq!(
                BackgroundAnimation::starting_for(theme),
                if theme == ThemePreset::Vercel {
                    BackgroundAnimation::Off
                } else {
                    BackgroundAnimation::Life
                }
            );
        }
    }
    #[test]
    fn pudding_persists_and_keeps_every_saved_name_stable() {
        for (name, style) in [
            ("\"curtains\"", BackgroundAnimation::Curtains),
            ("\"rain\"", BackgroundAnimation::Rain),
            ("\"clouds\"", BackgroundAnimation::Clouds),
            ("\"falling_leaves\"", BackgroundAnimation::FallingLeaves),
            ("\"pudding\"", BackgroundAnimation::Pudding),
            ("\"window_curtains\"", BackgroundAnimation::WindowCurtains),
            ("\"seafloor\"", BackgroundAnimation::Seafloor),
        ] {
            assert_eq!(serde_json::to_string(&style).unwrap(), name);
            assert_eq!(
                serde_json::from_str::<BackgroundAnimation>(name).unwrap(),
                style
            );
        }
        // The renderer selects its shader from these numbers.
        assert_eq!(BackgroundAnimation::Curtains as u32, 7);
        assert_eq!(BackgroundAnimation::Rain as u32, 8);
        assert_eq!(BackgroundAnimation::FallingLeaves as u32, 10);
        assert_eq!(BackgroundAnimation::Pudding as u32, 11);
        assert_eq!(BackgroundAnimation::WindowCurtains as u32, 12);
        assert_eq!(BackgroundAnimation::Seafloor as u32, 13);
    }
    #[test]
    fn removed_styles_migrate_to_their_replacements() {
        assert_eq!(
            serde_json::from_str::<BackgroundAnimation>("\"waves\"").unwrap(),
            BackgroundAnimation::Liquid
        );
        assert_eq!(
            serde_json::from_str::<BackgroundAnimation>("\"orbit\"").unwrap(),
            BackgroundAnimation::FlutedGlass
        );
        assert_eq!(
            serde_json::to_string(&BackgroundAnimation::Liquid).unwrap(),
            "\"liquid\""
        );
        assert_eq!(
            serde_json::to_string(&BackgroundAnimation::FlutedGlass).unwrap(),
            "\"fluted_glass\""
        );
    }
}

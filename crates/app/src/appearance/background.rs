//! Background choice is independent of the palette; only Theme default follows it.
use super::ThemePreset;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    #[default]
    #[serde(other)]
    ThemeDefault = 0,
}

impl BackgroundAnimation {
    pub const ALL: [Self; 7] = [
        Self::ThemeDefault,
        Self::Off,
        Self::Life,
        Self::Liquid,
        Self::Contours,
        Self::Drift,
        Self::FlutedGlass,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::ThemeDefault => "Theme default",
            Self::Off => "Off",
            Self::Life => "Life",
            Self::Liquid => "Liquid",
            Self::Contours => "Contours",
            Self::Drift => "Drift",
            Self::FlutedGlass => "Fluted Glass",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ThemeDefault => "Life by default; a still backdrop for Vercel.",
            Self::Off => "A still, distraction-free workspace.",
            Self::Life => "Tiny cells growing and evolving together.",
            Self::Liquid => "Slow liquid folds with soft, glossy highlights.",
            Self::Contours => "Fine, flowing topographic lines.",
            Self::Drift => "Sparse particles drifting gently upward.",
            Self::FlutedGlass => "Ribbed glass with drifting light behind it.",
        }
    }

    pub fn resolve(self, theme: ThemePreset) -> Self {
        match self {
            Self::ThemeDefault if theme == ThemePreset::Vercel => Self::Off,
            Self::ThemeDefault => Self::Life,
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_choices_survive_every_theme_and_default_preserves_vercel_focus() {
        for theme in ThemePreset::ALL {
            for choice in BackgroundAnimation::ALL {
                assert_eq!(BackgroundAnimation::ALL[choice as usize], choice);
                if choice != BackgroundAnimation::ThemeDefault {
                    assert_eq!(choice.resolve(theme), choice);
                }
            }
            assert_eq!(
                BackgroundAnimation::ThemeDefault.resolve(theme),
                if theme == ThemePreset::Vercel {
                    BackgroundAnimation::Off
                } else {
                    BackgroundAnimation::Life
                }
            );
        }
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

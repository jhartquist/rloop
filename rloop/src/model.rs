use serde::{Deserialize, Deserializer, Serialize};
use strum::{Display, EnumString};

/// The Model type for task execution.
///
/// Case-insensitive parsing for CLI (via `EnumString`) and config files (via custom deserializer).
#[derive(Copy, Clone, Debug, Display, EnumString, PartialEq, Eq, Serialize)]
#[strum(ascii_case_insensitive)]
pub enum Model {
    Haiku,
    Sonnet,
    Opus,
}

impl Model {
    /// Returns the context window size in tokens for this model.
    #[must_use]
    pub const fn context_window(self) -> u64 {
        match self {
            Self::Haiku | Self::Sonnet | Self::Opus => 200_000,
        }
    }
}

/// Custom deserializer for case-insensitive parsing in config files.
impl<'de> Deserialize<'de> for Model {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_variants() {
        assert_eq!(Model::Haiku.to_string(), "Haiku");
        assert_eq!(Model::Sonnet.to_string(), "Sonnet");
        assert_eq!(Model::Opus.to_string(), "Opus");
    }

    #[test]
    fn deserialize_valid() {
        let h: Model = serde_yaml::from_str("Haiku").unwrap();
        assert_eq!(h, Model::Haiku);
        let s: Model = serde_yaml::from_str("Sonnet").unwrap();
        assert_eq!(s, Model::Sonnet);
        let o: Model = serde_yaml::from_str("Opus").unwrap();
        assert_eq!(o, Model::Opus);
    }

    #[test]
    fn deserialize_lowercase() {
        // Custom deserializer is case-insensitive
        let h: Model = serde_yaml::from_str("haiku").unwrap();
        assert_eq!(h, Model::Haiku);
        let s: Model = serde_yaml::from_str("sonnet").unwrap();
        assert_eq!(s, Model::Sonnet);
        let o: Model = serde_yaml::from_str("opus").unwrap();
        assert_eq!(o, Model::Opus);
    }

    #[test]
    fn deserialize_uppercase() {
        let h: Model = serde_yaml::from_str("HAIKU").unwrap();
        assert_eq!(h, Model::Haiku);
        let s: Model = serde_yaml::from_str("SONNET").unwrap();
        assert_eq!(s, Model::Sonnet);
        let o: Model = serde_yaml::from_str("OPUS").unwrap();
        assert_eq!(o, Model::Opus);
    }

    #[test]
    fn parse_from_str_case_insensitive() {
        use std::str::FromStr;

        assert_eq!(Model::from_str("haiku").unwrap(), Model::Haiku);
        assert_eq!(Model::from_str("Haiku").unwrap(), Model::Haiku);
        assert_eq!(Model::from_str("HAIKU").unwrap(), Model::Haiku);

        assert_eq!(Model::from_str("sonnet").unwrap(), Model::Sonnet);
        assert_eq!(Model::from_str("Sonnet").unwrap(), Model::Sonnet);
        assert_eq!(Model::from_str("SONNET").unwrap(), Model::Sonnet);

        assert_eq!(Model::from_str("opus").unwrap(), Model::Opus);
        assert_eq!(Model::from_str("Opus").unwrap(), Model::Opus);
        assert_eq!(Model::from_str("OPUS").unwrap(), Model::Opus);
    }

    #[test]
    fn deserialize_invalid() {
        let result: Result<Model, _> = serde_yaml::from_str("gpt4");
        assert!(result.is_err());
    }

    #[test]
    fn context_window_sizes() {
        assert_eq!(Model::Haiku.context_window(), 200_000);
        assert_eq!(Model::Sonnet.context_window(), 200_000);
        assert_eq!(Model::Opus.context_window(), 200_000);
    }
}

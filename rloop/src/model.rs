use serde::Deserialize;
use strum::Display;

#[derive(Copy, Clone, Debug, Display, Deserialize)]
pub enum Model {
    Haiku,
    Sonnet,
    Opus,
}

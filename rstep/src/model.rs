use strum::Display;

#[derive(Debug, Display, Copy, Clone)]
pub enum Model {
    Haiku,
    Sonnet,
    Opus,
}

/// Display density for the history view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every block fully expanded.
    Detail,
    /// Completed blocks compressed to roughly 5–6 lines; in-progress
    /// tool blocks stay in detail.
    Normal,
    /// Each block rendered as a single line.
    Overview,
}

impl Mode {
    pub fn cycle(self) -> Self {
        match self {
            Mode::Detail => Mode::Normal,
            Mode::Normal => Mode::Overview,
            Mode::Overview => Mode::Detail,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Detail => "detail",
            Mode::Normal => "normal",
            Mode::Overview => "overview",
        }
    }
}

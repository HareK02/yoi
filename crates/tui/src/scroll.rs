//! History-view scroll state.
//!
//! Anchored at the top: `top_offset` counts logical lines (pre-wrap) from
//! the top of the history buffer. `follow_tail` is sticky — when set, the
//! draw step recomputes `top_offset` every frame so the latest line is
//! pinned at the bottom of the viewport. New content appended at the tail
//! only shifts the visible range when `follow_tail` is true; when the
//! user has scrolled up manually, their view stays put.

pub struct Scroll {
    pub follow_tail: bool,
    pub top_offset: usize,
    /// Cached by the renderer so key handlers can page or jump without
    /// recomputing layout.
    pub area_height: u16,
    pub total_lines: usize,
    /// Wrap-aware: the smallest `top_offset` that still leaves the last
    /// logical line pinned to the bottom row. Scrolling past this would
    /// just show empty space, so it doubles as the clamp for manual
    /// scroll actions. Updated every frame by the renderer.
    pub tail_top_offset: usize,
    /// Line indices where each turn starts. Used for Ctrl-[/] navigation.
    pub turn_starts: Vec<usize>,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            follow_tail: true,
            top_offset: 0,
            area_height: 0,
            total_lines: 0,
            tail_top_offset: 0,
            turn_starts: Vec::new(),
        }
    }
}

impl Scroll {
    /// Maximum valid `top_offset` given the cached totals. Beyond this
    /// the viewport would show only blank space past the tail. Uses the
    /// wrap-aware offset so long CJK / wrapped lines stay visible.
    pub fn max_top_offset(&self) -> usize {
        self.tail_top_offset
    }

    pub fn scroll_up(&mut self, n: usize) {
        self.follow_tail = false;
        self.top_offset = self.top_offset.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize) {
        let max = self.max_top_offset();
        let next = self.top_offset.saturating_add(n).min(max);
        self.top_offset = next;
        if next >= max {
            self.follow_tail = true;
        }
    }

    pub fn page_up(&mut self) {
        let step = self.area_height.saturating_sub(1).max(1) as usize;
        self.scroll_up(step);
    }

    pub fn page_down(&mut self) {
        let step = self.area_height.saturating_sub(1).max(1) as usize;
        self.scroll_down(step);
    }

    pub fn to_top(&mut self) {
        self.follow_tail = false;
        self.top_offset = 0;
    }

    pub fn to_bottom(&mut self) {
        self.follow_tail = true;
    }

    pub fn jump_prev_turn(&mut self) {
        // Find the last turn start strictly above the current top.
        let current = self.top_offset;
        let target = self
            .turn_starts
            .iter()
            .rev()
            .copied()
            .find(|&start| start < current);
        if let Some(t) = target {
            self.follow_tail = false;
            self.top_offset = t;
        } else if !self.turn_starts.is_empty() {
            // Already at or above the first turn; pin to top.
            self.follow_tail = false;
            self.top_offset = 0;
        }
    }

    pub fn jump_next_turn(&mut self) {
        let current = self.top_offset;
        let target = self
            .turn_starts
            .iter()
            .copied()
            .find(|&start| start > current);
        if let Some(t) = target {
            self.follow_tail = false;
            let max = self.max_top_offset();
            self.top_offset = t.min(max);
            if self.top_offset >= max {
                self.follow_tail = true;
            }
        } else {
            self.to_bottom();
        }
    }
}

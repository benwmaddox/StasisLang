#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
}

impl WindowConfig {
    pub fn is_vertical(self) -> bool {
        self.height > self.width
    }
}


#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClockPrecision {
    Minute,
    Second,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClockState {
    pub date: String,
    pub time: String,
}

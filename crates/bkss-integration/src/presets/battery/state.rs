#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeState {
    Charging,
    Discharging,
    Empty,
    FullyCharged,
    PendingCharge,
    PendingDischarge,
    Unknown(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Battery {
    pub path: String,
    pub model: Option<String>,
    pub percentage: f64,
    pub state: ChargeState,
    pub time_to_empty_seconds: Option<u64>,
    pub time_to_full_seconds: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatteryState {
    pub batteries: Vec<Battery>,
    pub on_battery: Option<bool>,
}

use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationError {
    InvalidTiming,
    InvalidEndpoint,
    InvalidDistance,
}

impl fmt::Display for AnimationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTiming => f.write_str("invalid animation timing"),
            Self::InvalidEndpoint => f.write_str("invalid animation endpoint"),
            Self::InvalidDistance => f.write_str("invalid animation distance"),
        }
    }
}

impl Error for AnimationError {}
